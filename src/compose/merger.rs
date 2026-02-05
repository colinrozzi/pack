//! Module merging logic.
//!
//! Handles merging multiple WASM modules into one, including:
//! - Type deduplication
//! - Index remapping for functions, types, globals, etc.
//! - Resolving imports to internal calls
//! - Memory and data segment merging

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use wasm_encoder::{
    CodeSection, ConstExpr as EncConstExpr, DataSection, ElementSection, Elements,
    ExportKind as EncExportKind, ExportSection, Function, FunctionSection, GlobalSection,
    GlobalType, HeapType, ImportSection, Instruction, MemorySection, MemoryType, Module, RefType,
    TableSection, TableType, TypeSection, ValType,
};

use super::error::ComposeError;
use super::parser::{
    BlockType, ConstExpr, DataSegment, DataSegmentKind, Element, ElementItems, ElementKind,
    ExportKind, FunctionCode, Global, ImportKind, MemArg, ParsedModule, RefTypeKind,
    StoredOperator,
};

/// Wiring specification: how to resolve an import.
#[derive(Debug, Clone)]
pub struct Wiring {
    /// The module that has the import.
    pub consumer: String,
    /// The import module name (as declared in the WASM import).
    pub import_module: String,
    /// The import function name.
    pub import_fn: String,
    /// The module providing the implementation.
    pub provider: String,
    /// The export name in the provider module.
    pub provider_export: String,
}

/// Export specification: what to export from the merged module.
#[derive(Debug, Clone)]
pub struct ExportSpec {
    /// Name of the export in the merged module.
    pub name: String,
    /// Source module name.
    pub source_module: String,
    /// Export name in the source module.
    pub source_export: String,
}

/// The merger combines multiple parsed modules into a single WASM module.
pub struct Merger {
    modules: Vec<ParsedModule>,
    wirings: Vec<Wiring>,
    exports: Vec<ExportSpec>,
}

/// Index remapping for a single module when merged.
#[derive(Debug, Default)]
struct IndexRemap {
    /// Old type index -> new type index.
    types: HashMap<u32, u32>,
    /// Old function index -> new function index.
    functions: HashMap<u32, u32>,
    /// Old table index -> new table index.
    tables: HashMap<u32, u32>,
    /// Old memory index -> new memory index.
    memories: HashMap<u32, u32>,
    /// Old global index -> new global index.
    globals: HashMap<u32, u32>,
}

impl Merger {
    /// Create a new merger.
    pub fn new(modules: Vec<ParsedModule>, wirings: Vec<Wiring>, exports: Vec<ExportSpec>) -> Self {
        Self {
            modules,
            wirings,
            exports,
        }
    }

    /// Merge the modules and produce a combined WASM binary.
    pub fn merge(self) -> Result<Vec<u8>, ComposeError> {
        if self.modules.is_empty() {
            return Err(ComposeError::NoModules);
        }

        // Step 1: Build dependency graph and topologically sort modules
        let sorted_modules = self.topological_sort()?;

        // Step 2: Build combined structures with remapping
        let mut merged = MergedModule::new();

        // Maps to track where things end up
        let mut module_remaps: HashMap<String, IndexRemap> = HashMap::new();
        let mut resolved_imports: HashMap<(String, String, String), u32> = HashMap::new();

        // Step 3: Collect all types and deduplicate
        for module in &sorted_modules {
            let mut remap = IndexRemap::default();
            for (old_idx, func_type) in module.types.iter().enumerate() {
                let new_idx = merged.add_type(func_type);
                remap.types.insert(old_idx as u32, new_idx);
            }
            module_remaps.insert(module.name.clone(), remap);
        }

        // Step 4: Process imports and figure out which become internal calls
        // First pass: collect all wiring info
        let mut import_resolutions: HashMap<(String, String, String), (String, String)> =
            HashMap::new();
        for wiring in &self.wirings {
            import_resolutions.insert(
                (
                    wiring.consumer.clone(),
                    wiring.import_module.clone(),
                    wiring.import_fn.clone(),
                ),
                (wiring.provider.clone(), wiring.provider_export.clone()),
            );
        }

        // Step 5: Process imports from ALL modules first
        // This is critical: WASM requires all imports before all defined functions,
        // so we must collect all imports first to know the total import count
        // before assigning indices to defined functions.
        for module in &sorted_modules {
            let remap = module_remaps.get_mut(&module.name).unwrap();

            // Process imports
            let mut import_func_idx = 0u32;
            let mut import_table_idx = 0u32;
            let mut import_memory_idx = 0u32;
            let mut import_global_idx = 0u32;

            for import in &module.imports {
                match &import.kind {
                    ImportKind::Function(type_idx) => {
                        let old_func_idx = import_func_idx;
                        import_func_idx += 1;

                        let key = (
                            module.name.clone(),
                            import.module.clone(),
                            import.name.clone(),
                        );

                        if import_resolutions.contains_key(&key) {
                            // This import is resolved internally - we'll fix the index later
                            // after we've processed the provider
                            let placeholder = u32::MAX; // Placeholder
                            remap.functions.insert(old_func_idx, placeholder);
                            resolved_imports.insert(key, old_func_idx);
                        } else {
                            // Keep as external import
                            let new_type_idx = remap.types[type_idx];
                            let new_func_idx = merged.add_import_function(
                                &import.module,
                                &import.name,
                                new_type_idx,
                            );
                            remap.functions.insert(old_func_idx, new_func_idx);
                        }
                    }
                    ImportKind::Table(table_ty) => {
                        let old_idx = import_table_idx;
                        import_table_idx += 1;
                        let new_idx =
                            merged.add_import_table(&import.module, &import.name, table_ty);
                        remap.tables.insert(old_idx, new_idx);
                    }
                    ImportKind::Memory(mem_ty) => {
                        let old_idx = import_memory_idx;
                        import_memory_idx += 1;
                        let new_idx =
                            merged.add_import_memory(&import.module, &import.name, mem_ty);
                        remap.memories.insert(old_idx, new_idx);
                    }
                    ImportKind::Global(global_ty) => {
                        let old_idx = import_global_idx;
                        import_global_idx += 1;
                        let new_idx =
                            merged.add_import_global(&import.module, &import.name, global_ty);
                        remap.globals.insert(old_idx, new_idx);
                    }
                }
            }
        }

        // Step 5b: Process defined entities (tables, memories, globals, functions)
        // Now that all imports are processed, we know the correct starting indices.
        for module in &sorted_modules {
            let remap = module_remaps.get_mut(&module.name).unwrap();

            // Add defined tables
            for (i, table_ty) in module.tables.iter().enumerate() {
                let old_idx = module.num_imported_tables + i as u32;
                let new_idx = merged.add_table(table_ty);
                remap.tables.insert(old_idx, new_idx);
            }

            // Add defined memories - SHARE memory across all modules
            // When modules are wired together, they need to share memory for
            // cross-module function calls to work (strings/data passed by pointer).
            // We map all module-defined memories to a single shared memory (index 0).
            for (i, mem_ty) in module.memories.iter().enumerate() {
                let old_idx = module.num_imported_memories + i as u32;
                // Add memory only if this is the first one we've seen
                let new_idx = if merged.memories.is_empty() {
                    merged.add_memory(mem_ty)
                } else {
                    // Expand the shared memory if this module needs more
                    merged.expand_memory(mem_ty);
                    0 // All modules share memory 0
                };
                remap.memories.insert(old_idx, new_idx);
            }

            // Add defined globals
            for (i, global) in module.globals.iter().enumerate() {
                let old_idx = module.num_imported_globals + i as u32;
                let (new_idx, _unified) = merged.add_global(global, remap);
                remap.globals.insert(old_idx, new_idx);
            }

            // Add defined functions (just declare types for now, code comes later)
            for (i, &type_idx) in module.functions.iter().enumerate() {
                let old_func_idx = module.num_imported_functions + i as u32;
                let new_type_idx = remap.types[&type_idx];
                let new_func_idx = merged.add_function(new_type_idx);
                remap.functions.insert(old_func_idx, new_func_idx);
            }
        }

        // Step 6: Resolve internal wirings now that we know all function indices
        for wiring in &self.wirings {
            let provider_module = sorted_modules
                .iter()
                .find(|m| m.name == wiring.provider)
                .ok_or_else(|| ComposeError::ModuleNotFound(wiring.provider.clone()))?;

            let export = provider_module
                .get_export(&wiring.provider_export)
                .ok_or_else(|| ComposeError::FunctionNotFound {
                    module: wiring.provider.clone(),
                    function: wiring.provider_export.clone(),
                })?;

            if export.kind != ExportKind::Function {
                return Err(ComposeError::FunctionNotFound {
                    module: wiring.provider.clone(),
                    function: wiring.provider_export.clone(),
                });
            }

            let provider_remap = &module_remaps[&wiring.provider];
            let new_func_idx = provider_remap.functions[&export.index];

            // Update the consumer's remap for this import
            let consumer_remap = module_remaps.get_mut(&wiring.consumer).unwrap();
            let key = (
                wiring.consumer.clone(),
                wiring.import_module.clone(),
                wiring.import_fn.clone(),
            );
            if let Some(&old_import_idx) = resolved_imports.get(&key) {
                consumer_remap.functions.insert(old_import_idx, new_func_idx);
            }
        }

        // Step 7: Add function bodies with remapped indices
        for module in &sorted_modules {
            let remap = &module_remaps[&module.name];
            for code in &module.code {
                let func = remap_function_body(code, remap);
                merged.add_code(func);
            }
        }

        // Step 8: Add data segments with relocation to avoid overlap
        // First module's data stays at original offsets, subsequent modules are relocated
        let mut cumulative_data_offset = 0u32;
        for module in &sorted_modules {
            let remap = &module_remaps[&module.name];
            for data in &module.data {
                merged.add_data(data, remap, cumulative_data_offset);
            }
            // After adding this module's data, update the offset for the next module
            // Align to 8-byte boundary for safety
            cumulative_data_offset = (merged.highest_data_addr + 7) & !7;
        }

        // Step 9: Add element segments
        for module in &sorted_modules {
            let remap = &module_remaps[&module.name];
            for elem in &module.elements {
                merged.add_element(elem, remap);
            }
        }

        // Step 10: Add exports
        for export_spec in &self.exports {
            let source_module = sorted_modules
                .iter()
                .find(|m| m.name == export_spec.source_module)
                .ok_or_else(|| ComposeError::ModuleNotFound(export_spec.source_module.clone()))?;

            let export = source_module
                .get_export(&export_spec.source_export)
                .ok_or_else(|| ComposeError::FunctionNotFound {
                    module: export_spec.source_module.clone(),
                    function: export_spec.source_export.clone(),
                })?;

            let remap = &module_remaps[&export_spec.source_module];
            let new_idx = match export.kind {
                ExportKind::Function => remap.functions[&export.index],
                ExportKind::Table => remap.tables[&export.index],
                ExportKind::Memory => remap.memories[&export.index],
                ExportKind::Global => remap.globals[&export.index],
            };

            merged.add_export(&export_spec.name, export.kind, new_idx);
        }

        // Step 11: Encode the merged module
        merged.encode()
    }

    /// Topologically sort modules so providers come before consumers.
    fn topological_sort(&self) -> Result<Vec<&ParsedModule>, ComposeError> {
        let module_map: HashMap<&str, &ParsedModule> =
            self.modules.iter().map(|m| (m.name.as_str(), m)).collect();

        // Build dependency graph: consumer -> providers it depends on
        let mut deps: HashMap<&str, HashSet<&str>> = HashMap::new();
        for module in &self.modules {
            deps.insert(&module.name, HashSet::new());
        }

        for wiring in &self.wirings {
            if let Some(dep_set) = deps.get_mut(wiring.consumer.as_str()) {
                if module_map.contains_key(wiring.provider.as_str()) {
                    dep_set.insert(wiring.provider.as_str());
                }
            }
        }

        // Kahn's algorithm for topological sort
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for module in &self.modules {
            in_degree.insert(&module.name, 0);
        }
        for (_, providers) in &deps {
            for provider in providers {
                *in_degree.get_mut(provider).unwrap() += 1;
            }
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&name, _)| name)
            .collect();

        let mut result = Vec::new();
        while let Some(name) = queue.pop() {
            result.push(module_map[name]);
            if let Some(providers) = deps.get(name) {
                for provider in providers {
                    let deg = in_degree.get_mut(provider).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(provider);
                    }
                }
            }
        }

        if result.len() != self.modules.len() {
            return Err(ComposeError::CircularDependency {
                cycle: self.modules.iter().map(|m| m.name.clone()).collect(),
            });
        }

        // Reverse so providers come first
        result.reverse();
        Ok(result)
    }
}

/// The merged module being built.
struct MergedModule {
    types: Vec<wasmparser::FuncType>,
    type_dedup: HashMap<TypeKey, u32>,

    imports: Vec<MergedImport>,
    num_imported_functions: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,

    functions: Vec<u32>, // type indices
    tables: Vec<wasmparser::TableType>,
    memories: Vec<wasmparser::MemoryType>,
    globals: Vec<(wasmparser::GlobalType, ConstExpr)>,
    exports: Vec<(String, ExportKind, u32)>,
    code: Vec<Function>,
    data: Vec<(DataSegmentKind, Vec<u8>)>,
    elements: Vec<MergedElement>,

    /// Highest data segment end address.
    /// Used to relocate subsequent modules' data to avoid overlap.
    highest_data_addr: u32,

    /// Index of the unified heap pointer global (mut i32, initial 0xC000).
    /// When modules share memory, they must share this global so allocations
    /// from different modules don't overlap.
    unified_heap_ptr: Option<u32>,

    /// Index of the unified encoding stack pointer global (mut i32, initial 0xB000).
    /// This is used for CGRF tuple encoding and must be shared when modules
    /// share memory.
    unified_enc_tuple_sp: Option<u32>,
}

struct MergedImport {
    module: String,
    name: String,
    kind: ImportKind,
}

struct MergedElement {
    kind: ElementKind,
    items: Vec<u32>, // Remapped function indices
}

/// Key for type deduplication.
#[derive(Hash, Eq, PartialEq)]
struct TypeKey {
    params: Vec<u8>,
    results: Vec<u8>,
}

impl TypeKey {
    fn from_func_type(ty: &wasmparser::FuncType) -> Self {
        Self {
            params: ty.params().iter().map(val_type_byte).collect(),
            results: ty.results().iter().map(val_type_byte).collect(),
        }
    }
}

fn val_type_byte(ty: &wasmparser::ValType) -> u8 {
    match ty {
        wasmparser::ValType::I32 => 0x7F,
        wasmparser::ValType::I64 => 0x7E,
        wasmparser::ValType::F32 => 0x7D,
        wasmparser::ValType::F64 => 0x7C,
        wasmparser::ValType::V128 => 0x7B,
        wasmparser::ValType::Ref(r) => {
            if r.is_func_ref() {
                0x70
            } else {
                0x6F
            }
        }
    }
}

impl MergedModule {
    fn new() -> Self {
        Self {
            types: Vec::new(),
            type_dedup: HashMap::new(),
            imports: Vec::new(),
            num_imported_functions: 0,
            num_imported_tables: 0,
            num_imported_memories: 0,
            num_imported_globals: 0,
            functions: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            exports: Vec::new(),
            code: Vec::new(),
            data: Vec::new(),
            elements: Vec::new(),
            highest_data_addr: 0,
            unified_heap_ptr: None,
            unified_enc_tuple_sp: None,
        }
    }

    fn add_type(&mut self, func_type: &wasmparser::FuncType) -> u32 {
        let key = TypeKey::from_func_type(func_type);
        if let Some(&idx) = self.type_dedup.get(&key) {
            return idx;
        }
        let idx = self.types.len() as u32;
        self.types.push(func_type.clone());
        self.type_dedup.insert(key, idx);
        idx
    }

    fn next_function_index(&self) -> u32 {
        self.num_imported_functions + self.functions.len() as u32
    }

    fn next_table_index(&self) -> u32 {
        self.num_imported_tables + self.tables.len() as u32
    }

    fn next_memory_index(&self) -> u32 {
        self.num_imported_memories + self.memories.len() as u32
    }

    fn next_global_index(&self) -> u32 {
        self.num_imported_globals + self.globals.len() as u32
    }

    fn add_import_function(&mut self, module: &str, name: &str, type_idx: u32) -> u32 {
        let idx = self.num_imported_functions;
        self.num_imported_functions += 1;
        self.imports.push(MergedImport {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Function(type_idx),
        });
        idx
    }

    fn add_import_table(
        &mut self,
        module: &str,
        name: &str,
        table_ty: &wasmparser::TableType,
    ) -> u32 {
        let idx = self.num_imported_tables;
        self.num_imported_tables += 1;
        self.imports.push(MergedImport {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Table(*table_ty),
        });
        idx
    }

    fn add_import_memory(
        &mut self,
        module: &str,
        name: &str,
        mem_ty: &wasmparser::MemoryType,
    ) -> u32 {
        let idx = self.num_imported_memories;
        self.num_imported_memories += 1;
        self.imports.push(MergedImport {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Memory(*mem_ty),
        });
        idx
    }

    fn add_import_global(
        &mut self,
        module: &str,
        name: &str,
        global_ty: &wasmparser::GlobalType,
    ) -> u32 {
        let idx = self.num_imported_globals;
        self.num_imported_globals += 1;
        self.imports.push(MergedImport {
            module: module.to_string(),
            name: name.to_string(),
            kind: ImportKind::Global(*global_ty),
        });
        idx
    }

    fn add_table(&mut self, table_ty: &wasmparser::TableType) -> u32 {
        let idx = self.next_table_index();
        self.tables.push(*table_ty);
        idx
    }

    fn add_memory(&mut self, mem_ty: &wasmparser::MemoryType) -> u32 {
        let idx = self.next_memory_index();
        self.memories.push(*mem_ty);
        idx
    }

    /// Expand the shared memory (index 0) to accommodate a larger requirement.
    /// Takes the maximum of the current and new memory limits.
    fn expand_memory(&mut self, mem_ty: &wasmparser::MemoryType) {
        if self.memories.is_empty() {
            return;
        }
        let shared = &mut self.memories[0];
        // Take the maximum initial and maximum sizes
        shared.initial = shared.initial.max(mem_ty.initial);
        shared.maximum = match (shared.maximum, mem_ty.maximum) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
    }

    /// Add a global, unifying heap-related globals when modules share memory.
    ///
    /// Wisp-compiled modules have two critical globals that must be shared
    /// when modules share memory:
    /// - Heap pointer ($__heap_ptr): mut i32, initial value 0xC000 (49152)
    /// - Encoding stack pointer ($enc_tuple_sp): mut i32, initial value 0xB000 (45056)
    ///
    /// Without unification, each module would allocate from the same starting
    /// address, causing memory corruption.
    fn add_global(&mut self, global: &Global, _remap: &IndexRemap) -> (u32, bool) {
        // Check if this is a heap pointer global (mut i32, value 49152)
        if self.is_heap_ptr_global(global) {
            if let Some(unified_idx) = self.unified_heap_ptr {
                // Return the existing unified heap pointer
                return (unified_idx, true);
            } else {
                // First heap pointer - add it and track
                let idx = self.next_global_index();
                self.globals.push((global.ty, global.init_expr.clone()));
                self.unified_heap_ptr = Some(idx);
                return (idx, false);
            }
        }

        // Check if this is an enc_tuple_sp global (mut i32, value 45056)
        if self.is_enc_tuple_sp_global(global) {
            if let Some(unified_idx) = self.unified_enc_tuple_sp {
                // Return the existing unified enc_tuple_sp
                return (unified_idx, true);
            } else {
                // First enc_tuple_sp - add it and track
                let idx = self.next_global_index();
                self.globals.push((global.ty, global.init_expr.clone()));
                self.unified_enc_tuple_sp = Some(idx);
                return (idx, false);
            }
        }

        // Regular global - add as new
        let idx = self.next_global_index();
        self.globals.push((global.ty, global.init_expr.clone()));
        (idx, false)
    }

    /// Check if a global is the heap pointer (mut i32, value 0xC000).
    fn is_heap_ptr_global(&self, global: &Global) -> bool {
        if !global.ty.mutable {
            return false;
        }
        if global.ty.content_type != wasmparser::ValType::I32 {
            return false;
        }
        matches!(global.init_expr, ConstExpr::I32Const(49152))
    }

    /// Check if a global is the encoding tuple stack pointer (mut i32, value 0xB000).
    fn is_enc_tuple_sp_global(&self, global: &Global) -> bool {
        if !global.ty.mutable {
            return false;
        }
        if global.ty.content_type != wasmparser::ValType::I32 {
            return false;
        }
        matches!(global.init_expr, ConstExpr::I32Const(45056))
    }

    fn add_function(&mut self, type_idx: u32) -> u32 {
        let idx = self.next_function_index();
        self.functions.push(type_idx);
        idx
    }

    fn add_code(&mut self, func: Function) {
        self.code.push(func);
    }

    /// Add a data segment with relocation support.
    /// The `data_offset` parameter specifies how much to offset this segment
    /// to avoid overlap with other modules' data.
    fn add_data(&mut self, data: &DataSegment, remap: &IndexRemap, data_offset: u32) {
        let kind = match &data.kind {
            DataSegmentKind::Active {
                memory_index,
                offset_expr,
            } => {
                let new_mem_idx = remap.memories.get(memory_index).copied().unwrap_or(*memory_index);
                // Apply data offset if this is a constant offset
                let new_offset = match offset_expr {
                    ConstExpr::I32Const(orig_offset) => {
                        let relocated = (*orig_offset as u32).saturating_add(data_offset);
                        // Track highest data address
                        let data_end = relocated + data.data.len() as u32;
                        if data_end > self.highest_data_addr {
                            self.highest_data_addr = data_end;
                        }
                        ConstExpr::I32Const(relocated as i32)
                    }
                    _ => {
                        // For non-constant offsets, we can't easily relocate
                        // Just track the data size for highest_data_addr
                        self.highest_data_addr = self.highest_data_addr.max(data.data.len() as u32);
                        offset_expr.clone()
                    }
                };
                DataSegmentKind::Active {
                    memory_index: new_mem_idx,
                    offset_expr: new_offset,
                }
            }
            DataSegmentKind::Passive => DataSegmentKind::Passive,
        };
        self.data.push((kind, data.data.clone()));
    }

    fn add_element(&mut self, elem: &Element, remap: &IndexRemap) {
        let items = match &elem.items {
            ElementItems::Functions(funcs) => funcs
                .iter()
                .map(|&idx| remap.functions.get(&idx).copied().unwrap_or(idx))
                .collect(),
            ElementItems::Expressions(_) => {
                // For expression-based elements, we'd need more complex handling
                // For now, skip these
                return;
            }
        };

        let kind = match &elem.kind {
            ElementKind::Active {
                table_index,
                offset_expr,
            } => {
                let new_table_idx = remap.tables.get(table_index).copied().unwrap_or(*table_index);
                ElementKind::Active {
                    table_index: new_table_idx,
                    offset_expr: offset_expr.clone(),
                }
            }
            ElementKind::Passive => ElementKind::Passive,
            ElementKind::Declared => ElementKind::Declared,
        };

        self.elements.push(MergedElement { kind, items });
    }

    fn add_export(&mut self, name: &str, kind: ExportKind, index: u32) {
        self.exports.push((name.to_string(), kind, index));
    }

    fn encode(self) -> Result<Vec<u8>, ComposeError> {
        let mut module = Module::new();

        // Type section
        if !self.types.is_empty() {
            let mut types = TypeSection::new();
            for func_type in &self.types {
                let params: Vec<ValType> = func_type
                    .params()
                    .iter()
                    .map(convert_val_type)
                    .collect();
                let results: Vec<ValType> = func_type
                    .results()
                    .iter()
                    .map(convert_val_type)
                    .collect();
                types.ty().function(params, results);
            }
            module.section(&types);
        }

        // Import section
        if !self.imports.is_empty() {
            let mut imports = ImportSection::new();
            for import in &self.imports {
                match &import.kind {
                    ImportKind::Function(type_idx) => {
                        imports.import(
                            &import.module,
                            &import.name,
                            wasm_encoder::EntityType::Function(*type_idx),
                        );
                    }
                    ImportKind::Table(table_ty) => {
                        let enc_ty = convert_table_type(table_ty);
                        imports.import(&import.module, &import.name, enc_ty);
                    }
                    ImportKind::Memory(mem_ty) => {
                        let enc_ty = convert_memory_type(mem_ty);
                        imports.import(&import.module, &import.name, enc_ty);
                    }
                    ImportKind::Global(global_ty) => {
                        let enc_ty = convert_global_type(global_ty);
                        imports.import(&import.module, &import.name, enc_ty);
                    }
                }
            }
            module.section(&imports);
        }

        // Function section
        if !self.functions.is_empty() {
            let mut functions = FunctionSection::new();
            for &type_idx in &self.functions {
                functions.function(type_idx);
            }
            module.section(&functions);
        }

        // Table section
        if !self.tables.is_empty() {
            let mut tables = TableSection::new();
            for table_ty in &self.tables {
                let enc_ty = convert_table_type(table_ty);
                tables.table(enc_ty);
            }
            module.section(&tables);
        }

        // Memory section
        if !self.memories.is_empty() {
            let mut memories = MemorySection::new();
            for mem_ty in &self.memories {
                let enc_ty = convert_memory_type(mem_ty);
                memories.memory(enc_ty);
            }
            module.section(&memories);
        }

        // Global section
        if !self.globals.is_empty() {
            let mut globals = GlobalSection::new();
            for (global_ty, init_expr) in &self.globals {
                let enc_ty = convert_global_type(global_ty);
                let enc_expr = convert_const_expr(init_expr);
                globals.global(enc_ty, &enc_expr);
            }
            module.section(&globals);
        }

        // Export section
        if !self.exports.is_empty() {
            let mut exports = ExportSection::new();
            for (name, kind, index) in &self.exports {
                let enc_kind = match kind {
                    ExportKind::Function => EncExportKind::Func,
                    ExportKind::Table => EncExportKind::Table,
                    ExportKind::Memory => EncExportKind::Memory,
                    ExportKind::Global => EncExportKind::Global,
                };
                exports.export(name, enc_kind, *index);
            }
            module.section(&exports);
        }

        // Element section
        if !self.elements.is_empty() {
            let mut elements = ElementSection::new();
            for elem in &self.elements {
                match &elem.kind {
                    ElementKind::Active {
                        table_index,
                        offset_expr,
                    } => {
                        let offset = convert_const_expr(offset_expr);
                        elements.active(
                            Some(*table_index),
                            &offset,
                            Elements::Functions(Cow::Borrowed(&elem.items)),
                        );
                    }
                    ElementKind::Passive => {
                        elements.passive(Elements::Functions(Cow::Borrowed(&elem.items)));
                    }
                    ElementKind::Declared => {
                        elements.declared(Elements::Functions(Cow::Borrowed(&elem.items)));
                    }
                }
            }
            module.section(&elements);
        }

        // Code section
        if !self.code.is_empty() {
            let mut code = CodeSection::new();
            for func in &self.code {
                code.function(func);
            }
            module.section(&code);
        }

        // Data section
        if !self.data.is_empty() {
            let mut data = DataSection::new();
            for (kind, bytes) in &self.data {
                match kind {
                    DataSegmentKind::Active {
                        memory_index,
                        offset_expr,
                    } => {
                        let offset = convert_const_expr(offset_expr);
                        data.active(*memory_index, &offset, bytes.iter().copied());
                    }
                    DataSegmentKind::Passive => {
                        data.passive(bytes.iter().copied());
                    }
                }
            }
            module.section(&data);
        }

        Ok(module.finish())
    }
}

/// Remap indices in a function body.
fn remap_function_body(code: &FunctionCode, remap: &IndexRemap) -> Function {
    let mut func = Function::new(
        code.locals
            .iter()
            .map(|(count, ty)| (*count, convert_val_type(ty))),
    );

    for op in &code.operators {
        let inst = convert_stored_operator(op, remap);
        func.instruction(&inst);
    }

    func
}

/// Convert a stored operator to a wasm-encoder instruction with index remapping.
fn convert_stored_operator(op: &StoredOperator, remap: &IndexRemap) -> Instruction<'static> {
    match op {
        // Control flow
        StoredOperator::Unreachable => Instruction::Unreachable,
        StoredOperator::Nop => Instruction::Nop,
        StoredOperator::Block(bt) => Instruction::Block(convert_block_type(bt, remap)),
        StoredOperator::Loop(bt) => Instruction::Loop(convert_block_type(bt, remap)),
        StoredOperator::If(bt) => Instruction::If(convert_block_type(bt, remap)),
        StoredOperator::Else => Instruction::Else,
        StoredOperator::End => Instruction::End,
        StoredOperator::Br(depth) => Instruction::Br(*depth),
        StoredOperator::BrIf(depth) => Instruction::BrIf(*depth),
        StoredOperator::BrTable { targets, default } => {
            Instruction::BrTable(Cow::Owned(targets.clone()), *default)
        }
        StoredOperator::Return => Instruction::Return,
        StoredOperator::Call(idx) => {
            let new_idx = remap.functions.get(idx).copied().unwrap_or(*idx);
            Instruction::Call(new_idx)
        }
        StoredOperator::CallIndirect {
            type_index,
            table_index,
        } => {
            let new_type = remap.types.get(type_index).copied().unwrap_or(*type_index);
            let new_table = remap.tables.get(table_index).copied().unwrap_or(*table_index);
            Instruction::CallIndirect {
                type_index: new_type,
                table_index: new_table,
            }
        }
        StoredOperator::ReturnCall(idx) => {
            let new_idx = remap.functions.get(idx).copied().unwrap_or(*idx);
            Instruction::ReturnCall(new_idx)
        }
        StoredOperator::ReturnCallIndirect {
            type_index,
            table_index,
        } => {
            let new_type = remap.types.get(type_index).copied().unwrap_or(*type_index);
            let new_table = remap.tables.get(table_index).copied().unwrap_or(*table_index);
            Instruction::ReturnCallIndirect {
                type_index: new_type,
                table_index: new_table,
            }
        }

        // Parametric
        StoredOperator::Drop => Instruction::Drop,
        StoredOperator::Select => Instruction::Select,
        StoredOperator::TypedSelect(ty) => Instruction::TypedSelect(convert_val_type(ty)),

        // Variable access
        StoredOperator::LocalGet(idx) => Instruction::LocalGet(*idx),
        StoredOperator::LocalSet(idx) => Instruction::LocalSet(*idx),
        StoredOperator::LocalTee(idx) => Instruction::LocalTee(*idx),
        StoredOperator::GlobalGet(idx) => {
            let new_idx = remap.globals.get(idx).copied().unwrap_or(*idx);
            Instruction::GlobalGet(new_idx)
        }
        StoredOperator::GlobalSet(idx) => {
            let new_idx = remap.globals.get(idx).copied().unwrap_or(*idx);
            Instruction::GlobalSet(new_idx)
        }

        // Table operations
        StoredOperator::TableGet(idx) => {
            let new_idx = remap.tables.get(idx).copied().unwrap_or(*idx);
            Instruction::TableGet(new_idx)
        }
        StoredOperator::TableSet(idx) => {
            let new_idx = remap.tables.get(idx).copied().unwrap_or(*idx);
            Instruction::TableSet(new_idx)
        }
        StoredOperator::TableGrow(idx) => {
            let new_idx = remap.tables.get(idx).copied().unwrap_or(*idx);
            Instruction::TableGrow(new_idx)
        }
        StoredOperator::TableSize(idx) => {
            let new_idx = remap.tables.get(idx).copied().unwrap_or(*idx);
            Instruction::TableSize(new_idx)
        }
        StoredOperator::TableFill(idx) => {
            let new_idx = remap.tables.get(idx).copied().unwrap_or(*idx);
            Instruction::TableFill(new_idx)
        }
        StoredOperator::TableCopy { dst, src } => {
            let new_dst = remap.tables.get(dst).copied().unwrap_or(*dst);
            let new_src = remap.tables.get(src).copied().unwrap_or(*src);
            Instruction::TableCopy {
                src_table: new_src,
                dst_table: new_dst,
            }
        }
        StoredOperator::TableInit { elem, table } => {
            let new_table = remap.tables.get(table).copied().unwrap_or(*table);
            Instruction::TableInit {
                elem_index: *elem,
                table: new_table,
            }
        }
        StoredOperator::ElemDrop(idx) => Instruction::ElemDrop(*idx),

        // Memory load operations
        StoredOperator::I32Load(m) => Instruction::I32Load(convert_memarg(m, remap)),
        StoredOperator::I64Load(m) => Instruction::I64Load(convert_memarg(m, remap)),
        StoredOperator::F32Load(m) => Instruction::F32Load(convert_memarg(m, remap)),
        StoredOperator::F64Load(m) => Instruction::F64Load(convert_memarg(m, remap)),
        StoredOperator::I32Load8S(m) => Instruction::I32Load8S(convert_memarg(m, remap)),
        StoredOperator::I32Load8U(m) => Instruction::I32Load8U(convert_memarg(m, remap)),
        StoredOperator::I32Load16S(m) => Instruction::I32Load16S(convert_memarg(m, remap)),
        StoredOperator::I32Load16U(m) => Instruction::I32Load16U(convert_memarg(m, remap)),
        StoredOperator::I64Load8S(m) => Instruction::I64Load8S(convert_memarg(m, remap)),
        StoredOperator::I64Load8U(m) => Instruction::I64Load8U(convert_memarg(m, remap)),
        StoredOperator::I64Load16S(m) => Instruction::I64Load16S(convert_memarg(m, remap)),
        StoredOperator::I64Load16U(m) => Instruction::I64Load16U(convert_memarg(m, remap)),
        StoredOperator::I64Load32S(m) => Instruction::I64Load32S(convert_memarg(m, remap)),
        StoredOperator::I64Load32U(m) => Instruction::I64Load32U(convert_memarg(m, remap)),

        // Memory store operations
        StoredOperator::I32Store(m) => Instruction::I32Store(convert_memarg(m, remap)),
        StoredOperator::I64Store(m) => Instruction::I64Store(convert_memarg(m, remap)),
        StoredOperator::F32Store(m) => Instruction::F32Store(convert_memarg(m, remap)),
        StoredOperator::F64Store(m) => Instruction::F64Store(convert_memarg(m, remap)),
        StoredOperator::I32Store8(m) => Instruction::I32Store8(convert_memarg(m, remap)),
        StoredOperator::I32Store16(m) => Instruction::I32Store16(convert_memarg(m, remap)),
        StoredOperator::I64Store8(m) => Instruction::I64Store8(convert_memarg(m, remap)),
        StoredOperator::I64Store16(m) => Instruction::I64Store16(convert_memarg(m, remap)),
        StoredOperator::I64Store32(m) => Instruction::I64Store32(convert_memarg(m, remap)),

        // Memory operations
        StoredOperator::MemorySize(idx) => {
            let new_idx = remap.memories.get(idx).copied().unwrap_or(*idx);
            Instruction::MemorySize(new_idx)
        }
        StoredOperator::MemoryGrow(idx) => {
            let new_idx = remap.memories.get(idx).copied().unwrap_or(*idx);
            Instruction::MemoryGrow(new_idx)
        }
        StoredOperator::MemoryInit { data, mem } => {
            let new_mem = remap.memories.get(mem).copied().unwrap_or(*mem);
            Instruction::MemoryInit {
                data_index: *data,
                mem: new_mem,
            }
        }
        StoredOperator::DataDrop(idx) => Instruction::DataDrop(*idx),
        StoredOperator::MemoryCopy { dst, src } => {
            let new_dst = remap.memories.get(dst).copied().unwrap_or(*dst);
            let new_src = remap.memories.get(src).copied().unwrap_or(*src);
            Instruction::MemoryCopy {
                src_mem: new_src,
                dst_mem: new_dst,
            }
        }
        StoredOperator::MemoryFill(idx) => {
            let new_idx = remap.memories.get(idx).copied().unwrap_or(*idx);
            Instruction::MemoryFill(new_idx)
        }

        // Constants
        StoredOperator::I32Const(v) => Instruction::I32Const(*v),
        StoredOperator::I64Const(v) => Instruction::I64Const(*v),
        StoredOperator::F32Const(v) => Instruction::F32Const(*v),
        StoredOperator::F64Const(v) => Instruction::F64Const(*v),

        // Comparison operators
        StoredOperator::I32Eqz => Instruction::I32Eqz,
        StoredOperator::I32Eq => Instruction::I32Eq,
        StoredOperator::I32Ne => Instruction::I32Ne,
        StoredOperator::I32LtS => Instruction::I32LtS,
        StoredOperator::I32LtU => Instruction::I32LtU,
        StoredOperator::I32GtS => Instruction::I32GtS,
        StoredOperator::I32GtU => Instruction::I32GtU,
        StoredOperator::I32LeS => Instruction::I32LeS,
        StoredOperator::I32LeU => Instruction::I32LeU,
        StoredOperator::I32GeS => Instruction::I32GeS,
        StoredOperator::I32GeU => Instruction::I32GeU,

        StoredOperator::I64Eqz => Instruction::I64Eqz,
        StoredOperator::I64Eq => Instruction::I64Eq,
        StoredOperator::I64Ne => Instruction::I64Ne,
        StoredOperator::I64LtS => Instruction::I64LtS,
        StoredOperator::I64LtU => Instruction::I64LtU,
        StoredOperator::I64GtS => Instruction::I64GtS,
        StoredOperator::I64GtU => Instruction::I64GtU,
        StoredOperator::I64LeS => Instruction::I64LeS,
        StoredOperator::I64LeU => Instruction::I64LeU,
        StoredOperator::I64GeS => Instruction::I64GeS,
        StoredOperator::I64GeU => Instruction::I64GeU,

        StoredOperator::F32Eq => Instruction::F32Eq,
        StoredOperator::F32Ne => Instruction::F32Ne,
        StoredOperator::F32Lt => Instruction::F32Lt,
        StoredOperator::F32Gt => Instruction::F32Gt,
        StoredOperator::F32Le => Instruction::F32Le,
        StoredOperator::F32Ge => Instruction::F32Ge,

        StoredOperator::F64Eq => Instruction::F64Eq,
        StoredOperator::F64Ne => Instruction::F64Ne,
        StoredOperator::F64Lt => Instruction::F64Lt,
        StoredOperator::F64Gt => Instruction::F64Gt,
        StoredOperator::F64Le => Instruction::F64Le,
        StoredOperator::F64Ge => Instruction::F64Ge,

        // Numeric operators - i32
        StoredOperator::I32Clz => Instruction::I32Clz,
        StoredOperator::I32Ctz => Instruction::I32Ctz,
        StoredOperator::I32Popcnt => Instruction::I32Popcnt,
        StoredOperator::I32Add => Instruction::I32Add,
        StoredOperator::I32Sub => Instruction::I32Sub,
        StoredOperator::I32Mul => Instruction::I32Mul,
        StoredOperator::I32DivS => Instruction::I32DivS,
        StoredOperator::I32DivU => Instruction::I32DivU,
        StoredOperator::I32RemS => Instruction::I32RemS,
        StoredOperator::I32RemU => Instruction::I32RemU,
        StoredOperator::I32And => Instruction::I32And,
        StoredOperator::I32Or => Instruction::I32Or,
        StoredOperator::I32Xor => Instruction::I32Xor,
        StoredOperator::I32Shl => Instruction::I32Shl,
        StoredOperator::I32ShrS => Instruction::I32ShrS,
        StoredOperator::I32ShrU => Instruction::I32ShrU,
        StoredOperator::I32Rotl => Instruction::I32Rotl,
        StoredOperator::I32Rotr => Instruction::I32Rotr,

        // Numeric operators - i64
        StoredOperator::I64Clz => Instruction::I64Clz,
        StoredOperator::I64Ctz => Instruction::I64Ctz,
        StoredOperator::I64Popcnt => Instruction::I64Popcnt,
        StoredOperator::I64Add => Instruction::I64Add,
        StoredOperator::I64Sub => Instruction::I64Sub,
        StoredOperator::I64Mul => Instruction::I64Mul,
        StoredOperator::I64DivS => Instruction::I64DivS,
        StoredOperator::I64DivU => Instruction::I64DivU,
        StoredOperator::I64RemS => Instruction::I64RemS,
        StoredOperator::I64RemU => Instruction::I64RemU,
        StoredOperator::I64And => Instruction::I64And,
        StoredOperator::I64Or => Instruction::I64Or,
        StoredOperator::I64Xor => Instruction::I64Xor,
        StoredOperator::I64Shl => Instruction::I64Shl,
        StoredOperator::I64ShrS => Instruction::I64ShrS,
        StoredOperator::I64ShrU => Instruction::I64ShrU,
        StoredOperator::I64Rotl => Instruction::I64Rotl,
        StoredOperator::I64Rotr => Instruction::I64Rotr,

        // Numeric operators - f32
        StoredOperator::F32Abs => Instruction::F32Abs,
        StoredOperator::F32Neg => Instruction::F32Neg,
        StoredOperator::F32Ceil => Instruction::F32Ceil,
        StoredOperator::F32Floor => Instruction::F32Floor,
        StoredOperator::F32Trunc => Instruction::F32Trunc,
        StoredOperator::F32Nearest => Instruction::F32Nearest,
        StoredOperator::F32Sqrt => Instruction::F32Sqrt,
        StoredOperator::F32Add => Instruction::F32Add,
        StoredOperator::F32Sub => Instruction::F32Sub,
        StoredOperator::F32Mul => Instruction::F32Mul,
        StoredOperator::F32Div => Instruction::F32Div,
        StoredOperator::F32Min => Instruction::F32Min,
        StoredOperator::F32Max => Instruction::F32Max,
        StoredOperator::F32Copysign => Instruction::F32Copysign,

        // Numeric operators - f64
        StoredOperator::F64Abs => Instruction::F64Abs,
        StoredOperator::F64Neg => Instruction::F64Neg,
        StoredOperator::F64Ceil => Instruction::F64Ceil,
        StoredOperator::F64Floor => Instruction::F64Floor,
        StoredOperator::F64Trunc => Instruction::F64Trunc,
        StoredOperator::F64Nearest => Instruction::F64Nearest,
        StoredOperator::F64Sqrt => Instruction::F64Sqrt,
        StoredOperator::F64Add => Instruction::F64Add,
        StoredOperator::F64Sub => Instruction::F64Sub,
        StoredOperator::F64Mul => Instruction::F64Mul,
        StoredOperator::F64Div => Instruction::F64Div,
        StoredOperator::F64Min => Instruction::F64Min,
        StoredOperator::F64Max => Instruction::F64Max,
        StoredOperator::F64Copysign => Instruction::F64Copysign,

        // Conversions
        StoredOperator::I32WrapI64 => Instruction::I32WrapI64,
        StoredOperator::I32TruncF32S => Instruction::I32TruncF32S,
        StoredOperator::I32TruncF32U => Instruction::I32TruncF32U,
        StoredOperator::I32TruncF64S => Instruction::I32TruncF64S,
        StoredOperator::I32TruncF64U => Instruction::I32TruncF64U,
        StoredOperator::I64ExtendI32S => Instruction::I64ExtendI32S,
        StoredOperator::I64ExtendI32U => Instruction::I64ExtendI32U,
        StoredOperator::I64TruncF32S => Instruction::I64TruncF32S,
        StoredOperator::I64TruncF32U => Instruction::I64TruncF32U,
        StoredOperator::I64TruncF64S => Instruction::I64TruncF64S,
        StoredOperator::I64TruncF64U => Instruction::I64TruncF64U,
        StoredOperator::F32ConvertI32S => Instruction::F32ConvertI32S,
        StoredOperator::F32ConvertI32U => Instruction::F32ConvertI32U,
        StoredOperator::F32ConvertI64S => Instruction::F32ConvertI64S,
        StoredOperator::F32ConvertI64U => Instruction::F32ConvertI64U,
        StoredOperator::F32DemoteF64 => Instruction::F32DemoteF64,
        StoredOperator::F64ConvertI32S => Instruction::F64ConvertI32S,
        StoredOperator::F64ConvertI32U => Instruction::F64ConvertI32U,
        StoredOperator::F64ConvertI64S => Instruction::F64ConvertI64S,
        StoredOperator::F64ConvertI64U => Instruction::F64ConvertI64U,
        StoredOperator::F64PromoteF32 => Instruction::F64PromoteF32,
        StoredOperator::I32ReinterpretF32 => Instruction::I32ReinterpretF32,
        StoredOperator::I64ReinterpretF64 => Instruction::I64ReinterpretF64,
        StoredOperator::F32ReinterpretI32 => Instruction::F32ReinterpretI32,
        StoredOperator::F64ReinterpretI64 => Instruction::F64ReinterpretI64,

        // Sign extension
        StoredOperator::I32Extend8S => Instruction::I32Extend8S,
        StoredOperator::I32Extend16S => Instruction::I32Extend16S,
        StoredOperator::I64Extend8S => Instruction::I64Extend8S,
        StoredOperator::I64Extend16S => Instruction::I64Extend16S,
        StoredOperator::I64Extend32S => Instruction::I64Extend32S,

        // Saturating truncation
        StoredOperator::I32TruncSatF32S => Instruction::I32TruncSatF32S,
        StoredOperator::I32TruncSatF32U => Instruction::I32TruncSatF32U,
        StoredOperator::I32TruncSatF64S => Instruction::I32TruncSatF64S,
        StoredOperator::I32TruncSatF64U => Instruction::I32TruncSatF64U,
        StoredOperator::I64TruncSatF32S => Instruction::I64TruncSatF32S,
        StoredOperator::I64TruncSatF32U => Instruction::I64TruncSatF32U,
        StoredOperator::I64TruncSatF64S => Instruction::I64TruncSatF64S,
        StoredOperator::I64TruncSatF64U => Instruction::I64TruncSatF64U,

        // Reference types
        StoredOperator::RefNull(kind) => {
            match kind {
                RefTypeKind::Func => Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: wasm_encoder::AbstractHeapType::Func,
                }),
                RefTypeKind::Extern => Instruction::RefNull(HeapType::Abstract {
                    shared: false,
                    ty: wasm_encoder::AbstractHeapType::Extern,
                }),
            }
        }
        StoredOperator::RefIsNull => Instruction::RefIsNull,
        StoredOperator::RefFunc(idx) => {
            let new_idx = remap.functions.get(idx).copied().unwrap_or(*idx);
            Instruction::RefFunc(new_idx)
        }
    }
}

fn convert_val_type(ty: &wasmparser::ValType) -> ValType {
    match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        wasmparser::ValType::V128 => ValType::V128,
        wasmparser::ValType::Ref(r) => {
            if r.is_func_ref() {
                ValType::Ref(RefType::FUNCREF)
            } else {
                ValType::Ref(RefType::EXTERNREF)
            }
        }
    }
}

fn convert_block_type(bt: &BlockType, remap: &IndexRemap) -> wasm_encoder::BlockType {
    match bt {
        BlockType::Empty => wasm_encoder::BlockType::Empty,
        BlockType::Type(ty) => wasm_encoder::BlockType::Result(convert_val_type(ty)),
        BlockType::FuncType(idx) => {
            let new_idx = remap.types.get(idx).copied().unwrap_or(*idx);
            wasm_encoder::BlockType::FunctionType(new_idx)
        }
    }
}

fn convert_memarg(m: &MemArg, remap: &IndexRemap) -> wasm_encoder::MemArg {
    let new_mem = remap.memories.get(&m.memory).copied().unwrap_or(m.memory);
    wasm_encoder::MemArg {
        offset: m.offset,
        align: m.align,
        memory_index: new_mem,
    }
}

fn convert_table_type(ty: &wasmparser::TableType) -> TableType {
    let ref_type = if ty.element_type.is_func_ref() {
        RefType::FUNCREF
    } else {
        RefType::EXTERNREF
    };
    TableType {
        element_type: ref_type,
        minimum: ty.initial,
        maximum: ty.maximum,
        table64: ty.table64,
        shared: ty.shared,
    }
}

fn convert_memory_type(ty: &wasmparser::MemoryType) -> MemoryType {
    MemoryType {
        minimum: ty.initial,
        maximum: ty.maximum,
        memory64: ty.memory64,
        shared: ty.shared,
        page_size_log2: ty.page_size_log2,
    }
}

fn convert_global_type(ty: &wasmparser::GlobalType) -> GlobalType {
    GlobalType {
        val_type: convert_val_type(&ty.content_type),
        mutable: ty.mutable,
        shared: ty.shared,
    }
}

fn convert_const_expr(expr: &ConstExpr) -> EncConstExpr {
    match expr {
        ConstExpr::I32Const(v) => EncConstExpr::i32_const(*v),
        ConstExpr::I64Const(v) => EncConstExpr::i64_const(*v),
        ConstExpr::F32Const(v) => EncConstExpr::f32_const(*v),
        ConstExpr::F64Const(v) => EncConstExpr::f64_const(*v),
        ConstExpr::GlobalGet(idx) => EncConstExpr::global_get(*idx),
        ConstExpr::RefNull(kind) => {
            let heap_type = match kind {
                RefTypeKind::Func => HeapType::Abstract {
                    shared: false,
                    ty: wasm_encoder::AbstractHeapType::Func,
                },
                RefTypeKind::Extern => HeapType::Abstract {
                    shared: false,
                    ty: wasm_encoder::AbstractHeapType::Extern,
                },
            };
            EncConstExpr::ref_null(heap_type)
        }
        ConstExpr::RefFunc(idx) => EncConstExpr::ref_func(*idx),
    }
}
