//! WASM module parsing utilities.
//!
//! Extracts types, imports, exports, functions, and other sections from WASM modules
//! for use in static composition.

use std::collections::HashMap;

use wasmparser::{
    BinaryReaderError, DataSectionReader, ElementSectionReader, ExportSectionReader,
    FunctionBody, FunctionSectionReader, GlobalSectionReader, ImportSectionReader,
    MemorySectionReader, Operator, Parser, Payload, TableSectionReader, TypeSectionReader,
};

use super::error::ComposeError;

/// Information about a parsed WASM module.
#[derive(Debug, Clone)]
pub struct ParsedModule {
    /// Module name (set by caller).
    pub name: String,

    /// Type section - function signatures.
    pub types: Vec<wasmparser::FuncType>,

    /// Imports.
    pub imports: Vec<Import>,

    /// Function type indices (indexes into types).
    pub functions: Vec<u32>,

    /// Tables.
    pub tables: Vec<wasmparser::TableType>,

    /// Memories.
    pub memories: Vec<wasmparser::MemoryType>,

    /// Globals.
    pub globals: Vec<Global>,

    /// Exports.
    pub exports: Vec<Export>,

    /// Function bodies (code section).
    pub code: Vec<FunctionCode>,

    /// Data segments.
    pub data: Vec<DataSegment>,

    /// Element segments (for indirect calls).
    pub elements: Vec<Element>,

    /// Start function index, if any.
    pub start: Option<u32>,

    /// Custom sections.
    pub custom_sections: Vec<CustomSection>,

    /// Number of imported functions (affects function index space).
    pub num_imported_functions: u32,

    /// Number of imported tables.
    pub num_imported_tables: u32,

    /// Number of imported memories.
    pub num_imported_memories: u32,

    /// Number of imported globals.
    pub num_imported_globals: u32,
}

/// An import declaration.
#[derive(Debug, Clone)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: ImportKind,
}

/// The kind of import.
#[derive(Debug, Clone)]
pub enum ImportKind {
    Function(u32), // type index
    Table(wasmparser::TableType),
    Memory(wasmparser::MemoryType),
    Global(wasmparser::GlobalType),
}

/// An export declaration.
#[derive(Debug, Clone)]
pub struct Export {
    pub name: String,
    pub kind: ExportKind,
    pub index: u32,
}

/// The kind of export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Function,
    Table,
    Memory,
    Global,
}

/// A global variable.
#[derive(Debug, Clone)]
pub struct Global {
    pub ty: wasmparser::GlobalType,
    pub init_expr: ConstExpr,
}

/// A function body from the code section.
#[derive(Debug, Clone)]
pub struct FunctionCode {
    /// Local variable types.
    pub locals: Vec<(u32, wasmparser::ValType)>,
    /// Parsed operators.
    pub operators: Vec<StoredOperator>,
}

/// A stored operator - simplified representation for merging.
#[derive(Debug, Clone)]
pub enum StoredOperator {
    // Control flow
    Unreachable,
    Nop,
    Block(BlockType),
    Loop(BlockType),
    If(BlockType),
    Else,
    End,
    Br(u32),
    BrIf(u32),
    BrTable { targets: Vec<u32>, default: u32 },
    Return,
    Call(u32),
    CallIndirect { type_index: u32, table_index: u32 },
    ReturnCall(u32),
    ReturnCallIndirect { type_index: u32, table_index: u32 },

    // Parametric
    Drop,
    Select,
    TypedSelect(wasmparser::ValType),

    // Variable access
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),

    // Table operations
    TableGet(u32),
    TableSet(u32),
    TableGrow(u32),
    TableSize(u32),
    TableFill(u32),
    TableCopy { dst: u32, src: u32 },
    TableInit { elem: u32, table: u32 },
    ElemDrop(u32),

    // Memory load operations
    I32Load(MemArg),
    I64Load(MemArg),
    F32Load(MemArg),
    F64Load(MemArg),
    I32Load8S(MemArg),
    I32Load8U(MemArg),
    I32Load16S(MemArg),
    I32Load16U(MemArg),
    I64Load8S(MemArg),
    I64Load8U(MemArg),
    I64Load16S(MemArg),
    I64Load16U(MemArg),
    I64Load32S(MemArg),
    I64Load32U(MemArg),

    // Memory store operations
    I32Store(MemArg),
    I64Store(MemArg),
    F32Store(MemArg),
    F64Store(MemArg),
    I32Store8(MemArg),
    I32Store16(MemArg),
    I64Store8(MemArg),
    I64Store16(MemArg),
    I64Store32(MemArg),

    // Memory operations
    MemorySize(u32),
    MemoryGrow(u32),
    MemoryInit { data: u32, mem: u32 },
    DataDrop(u32),
    MemoryCopy { dst: u32, src: u32 },
    MemoryFill(u32),

    // Constants
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),

    // Comparison operators
    I32Eqz, I32Eq, I32Ne, I32LtS, I32LtU, I32GtS, I32GtU, I32LeS, I32LeU, I32GeS, I32GeU,
    I64Eqz, I64Eq, I64Ne, I64LtS, I64LtU, I64GtS, I64GtU, I64LeS, I64LeU, I64GeS, I64GeU,
    F32Eq, F32Ne, F32Lt, F32Gt, F32Le, F32Ge,
    F64Eq, F64Ne, F64Lt, F64Gt, F64Le, F64Ge,

    // Numeric operators - i32
    I32Clz, I32Ctz, I32Popcnt, I32Add, I32Sub, I32Mul, I32DivS, I32DivU,
    I32RemS, I32RemU, I32And, I32Or, I32Xor, I32Shl, I32ShrS, I32ShrU, I32Rotl, I32Rotr,

    // Numeric operators - i64
    I64Clz, I64Ctz, I64Popcnt, I64Add, I64Sub, I64Mul, I64DivS, I64DivU,
    I64RemS, I64RemU, I64And, I64Or, I64Xor, I64Shl, I64ShrS, I64ShrU, I64Rotl, I64Rotr,

    // Numeric operators - f32
    F32Abs, F32Neg, F32Ceil, F32Floor, F32Trunc, F32Nearest, F32Sqrt,
    F32Add, F32Sub, F32Mul, F32Div, F32Min, F32Max, F32Copysign,

    // Numeric operators - f64
    F64Abs, F64Neg, F64Ceil, F64Floor, F64Trunc, F64Nearest, F64Sqrt,
    F64Add, F64Sub, F64Mul, F64Div, F64Min, F64Max, F64Copysign,

    // Conversions
    I32WrapI64,
    I32TruncF32S, I32TruncF32U, I32TruncF64S, I32TruncF64U,
    I64ExtendI32S, I64ExtendI32U,
    I64TruncF32S, I64TruncF32U, I64TruncF64S, I64TruncF64U,
    F32ConvertI32S, F32ConvertI32U, F32ConvertI64S, F32ConvertI64U, F32DemoteF64,
    F64ConvertI32S, F64ConvertI32U, F64ConvertI64S, F64ConvertI64U, F64PromoteF32,
    I32ReinterpretF32, I64ReinterpretF64, F32ReinterpretI32, F64ReinterpretI64,

    // Sign extension
    I32Extend8S, I32Extend16S,
    I64Extend8S, I64Extend16S, I64Extend32S,

    // Saturating truncation
    I32TruncSatF32S, I32TruncSatF32U, I32TruncSatF64S, I32TruncSatF64U,
    I64TruncSatF32S, I64TruncSatF32U, I64TruncSatF64S, I64TruncSatF64U,

    // Reference types
    RefNull(RefTypeKind),
    RefIsNull,
    RefFunc(u32),
}

/// Simplified block type.
#[derive(Debug, Clone, Copy)]
pub enum BlockType {
    Empty,
    Type(wasmparser::ValType),
    FuncType(u32),
}

/// Memory argument.
#[derive(Debug, Clone, Copy)]
pub struct MemArg {
    pub offset: u64,
    pub align: u32,
    pub memory: u32,
}

/// Reference type kind.
#[derive(Debug, Clone, Copy)]
pub enum RefTypeKind {
    Func,
    Extern,
}

/// Constant expression.
#[derive(Debug, Clone)]
pub enum ConstExpr {
    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),
    GlobalGet(u32),
    RefNull(RefTypeKind),
    RefFunc(u32),
}

/// A data segment.
#[derive(Debug, Clone)]
pub struct DataSegment {
    pub kind: DataSegmentKind,
    pub data: Vec<u8>,
}

/// The kind of data segment.
#[derive(Debug, Clone)]
pub enum DataSegmentKind {
    /// Active segment with memory index and offset expression.
    Active { memory_index: u32, offset_expr: ConstExpr },
    /// Passive segment.
    Passive,
}

/// An element segment for indirect calls.
#[derive(Debug, Clone)]
pub struct Element {
    pub kind: ElementKind,
    /// Function indices or expressions.
    pub items: ElementItems,
}

/// The kind of element segment.
#[derive(Debug, Clone)]
pub enum ElementKind {
    /// Active segment.
    Active {
        table_index: u32,
        offset_expr: ConstExpr,
    },
    /// Passive segment.
    Passive,
    /// Declared segment.
    Declared,
}

/// Items in an element segment.
#[derive(Debug, Clone)]
pub enum ElementItems {
    /// Function indices.
    Functions(Vec<u32>),
    /// Expressions (for ref types).
    Expressions(Vec<ConstExpr>),
}

/// A custom section.
#[derive(Debug, Clone)]
pub struct CustomSection {
    pub name: String,
    pub data: Vec<u8>,
}

impl ParsedModule {
    /// Parse a WASM module from bytes.
    pub fn parse(name: &str, wasm: &[u8]) -> Result<Self, ComposeError> {
        let mut module = ParsedModule {
            name: name.to_string(),
            types: Vec::new(),
            imports: Vec::new(),
            functions: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            exports: Vec::new(),
            code: Vec::new(),
            data: Vec::new(),
            elements: Vec::new(),
            start: None,
            custom_sections: Vec::new(),
            num_imported_functions: 0,
            num_imported_tables: 0,
            num_imported_memories: 0,
            num_imported_globals: 0,
        };

        let parser = Parser::new(0);
        for payload in parser.parse_all(wasm) {
            let payload = payload.map_err(|e| parse_error(name, e))?;
            module.process_payload(name, payload)?;
        }

        Ok(module)
    }

    fn process_payload(&mut self, name: &str, payload: Payload<'_>) -> Result<(), ComposeError> {
        match payload {
            Payload::TypeSection(reader) => self.parse_types(name, reader)?,
            Payload::ImportSection(reader) => self.parse_imports(name, reader)?,
            Payload::FunctionSection(reader) => self.parse_functions(name, reader)?,
            Payload::TableSection(reader) => self.parse_tables(name, reader)?,
            Payload::MemorySection(reader) => self.parse_memories(name, reader)?,
            Payload::GlobalSection(reader) => self.parse_globals(name, reader)?,
            Payload::ExportSection(reader) => self.parse_exports(name, reader)?,
            Payload::StartSection { func, .. } => self.start = Some(func),
            Payload::ElementSection(reader) => self.parse_elements(name, reader)?,
            Payload::CodeSectionEntry(body) => self.parse_code_entry(name, body)?,
            Payload::DataSection(reader) => self.parse_data(name, reader)?,
            Payload::CustomSection(reader) => {
                self.custom_sections.push(CustomSection {
                    name: reader.name().to_string(),
                    data: reader.data().to_vec(),
                });
            }
            _ => {} // Ignore other sections
        }
        Ok(())
    }

    fn parse_types(&mut self, name: &str, reader: TypeSectionReader<'_>) -> Result<(), ComposeError> {
        for rec_group in reader {
            let rec_group = rec_group.map_err(|e| parse_error(name, e))?;
            for ty in rec_group.into_types() {
                if let wasmparser::CompositeInnerType::Func(func_type) = ty.composite_type.inner {
                    self.types.push(func_type);
                }
            }
        }
        Ok(())
    }

    fn parse_imports(
        &mut self,
        name: &str,
        reader: ImportSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for import in reader {
            let import = import.map_err(|e| parse_error(name, e))?;
            let kind = match import.ty {
                wasmparser::TypeRef::Func(idx) => {
                    self.num_imported_functions += 1;
                    ImportKind::Function(idx)
                }
                wasmparser::TypeRef::Table(ty) => {
                    self.num_imported_tables += 1;
                    ImportKind::Table(ty)
                }
                wasmparser::TypeRef::Memory(ty) => {
                    self.num_imported_memories += 1;
                    ImportKind::Memory(ty)
                }
                wasmparser::TypeRef::Global(ty) => {
                    self.num_imported_globals += 1;
                    ImportKind::Global(ty)
                }
                wasmparser::TypeRef::Tag(_) => continue, // Skip exception handling
            };

            self.imports.push(Import {
                module: import.module.to_string(),
                name: import.name.to_string(),
                kind,
            });
        }
        Ok(())
    }

    fn parse_functions(
        &mut self,
        name: &str,
        reader: FunctionSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for func in reader {
            let type_idx = func.map_err(|e| parse_error(name, e))?;
            self.functions.push(type_idx);
        }
        Ok(())
    }

    fn parse_tables(
        &mut self,
        name: &str,
        reader: TableSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for table in reader {
            let table = table.map_err(|e| parse_error(name, e))?;
            self.tables.push(table.ty);
        }
        Ok(())
    }

    fn parse_memories(
        &mut self,
        name: &str,
        reader: MemorySectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for memory in reader {
            let memory = memory.map_err(|e| parse_error(name, e))?;
            self.memories.push(memory);
        }
        Ok(())
    }

    fn parse_globals(
        &mut self,
        name: &str,
        reader: GlobalSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for global in reader {
            let global = global.map_err(|e| parse_error(name, e))?;
            let init_expr = parse_const_expr(name, global.init_expr)?;
            self.globals.push(Global {
                ty: global.ty,
                init_expr,
            });
        }
        Ok(())
    }

    fn parse_exports(
        &mut self,
        name: &str,
        reader: ExportSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for export in reader {
            let export = export.map_err(|e| parse_error(name, e))?;
            let kind = match export.kind {
                wasmparser::ExternalKind::Func => ExportKind::Function,
                wasmparser::ExternalKind::Table => ExportKind::Table,
                wasmparser::ExternalKind::Memory => ExportKind::Memory,
                wasmparser::ExternalKind::Global => ExportKind::Global,
                wasmparser::ExternalKind::Tag => continue, // Skip exception handling
            };
            self.exports.push(Export {
                name: export.name.to_string(),
                kind,
                index: export.index,
            });
        }
        Ok(())
    }

    fn parse_elements(
        &mut self,
        name: &str,
        reader: ElementSectionReader<'_>,
    ) -> Result<(), ComposeError> {
        for elem in reader {
            let elem = elem.map_err(|e| parse_error(name, e))?;
            let kind = match elem.kind {
                wasmparser::ElementKind::Active {
                    table_index,
                    offset_expr,
                } => ElementKind::Active {
                    table_index: table_index.unwrap_or(0),
                    offset_expr: parse_const_expr(name, offset_expr)?,
                },
                wasmparser::ElementKind::Passive => ElementKind::Passive,
                wasmparser::ElementKind::Declared => ElementKind::Declared,
            };

            let items = match elem.items {
                wasmparser::ElementItems::Functions(reader) => {
                    let mut funcs = Vec::new();
                    for func in reader {
                        funcs.push(func.map_err(|e| parse_error(name, e))?);
                    }
                    ElementItems::Functions(funcs)
                }
                wasmparser::ElementItems::Expressions(_, reader) => {
                    let mut exprs = Vec::new();
                    for expr in reader {
                        let expr = expr.map_err(|e| parse_error(name, e))?;
                        exprs.push(parse_const_expr(name, expr)?);
                    }
                    ElementItems::Expressions(exprs)
                }
            };

            self.elements.push(Element { kind, items });
        }
        Ok(())
    }

    fn parse_code_entry(&mut self, name: &str, body: FunctionBody<'_>) -> Result<(), ComposeError> {
        let mut locals = Vec::new();
        let locals_reader = body.get_locals_reader().map_err(|e| parse_error(name, e))?;
        for local in locals_reader {
            let (count, ty) = local.map_err(|e| parse_error(name, e))?;
            locals.push((count, ty));
        }

        // Parse operators
        let mut operators = Vec::new();
        let ops_reader = body.get_operators_reader().map_err(|e| parse_error(name, e))?;
        for op in ops_reader {
            let op = op.map_err(|e| parse_error(name, e))?;
            if let Some(stored) = convert_operator(op) {
                operators.push(stored);
            }
        }

        self.code.push(FunctionCode { locals, operators });
        Ok(())
    }

    fn parse_data(&mut self, name: &str, reader: DataSectionReader<'_>) -> Result<(), ComposeError> {
        for data in reader {
            let data = data.map_err(|e| parse_error(name, e))?;
            let kind = match data.kind {
                wasmparser::DataKind::Active {
                    memory_index,
                    offset_expr,
                } => DataSegmentKind::Active {
                    memory_index,
                    offset_expr: parse_const_expr(name, offset_expr)?,
                },
                wasmparser::DataKind::Passive => DataSegmentKind::Passive,
            };
            self.data.push(DataSegment {
                kind,
                data: data.data.to_vec(),
            });
        }
        Ok(())
    }

    /// Get an export by name.
    pub fn get_export(&self, name: &str) -> Option<&Export> {
        self.exports.iter().find(|e| e.name == name)
    }

    /// Get all function exports.
    pub fn function_exports(&self) -> impl Iterator<Item = &Export> {
        self.exports.iter().filter(|e| e.kind == ExportKind::Function)
    }

    /// Get all function imports.
    pub fn function_imports(&self) -> impl Iterator<Item = &Import> {
        self.imports.iter().filter(|i| matches!(i.kind, ImportKind::Function(_)))
    }

    /// Get the type of a function by its index in the function index space.
    pub fn get_function_type(&self, func_idx: u32) -> Option<&wasmparser::FuncType> {
        if func_idx < self.num_imported_functions {
            // Imported function - get type from import
            let mut import_idx = 0;
            for import in &self.imports {
                if let ImportKind::Function(type_idx) = import.kind {
                    if import_idx == func_idx {
                        return self.types.get(type_idx as usize);
                    }
                    import_idx += 1;
                }
            }
            None
        } else {
            // Defined function - get type from function section
            let local_idx = (func_idx - self.num_imported_functions) as usize;
            self.functions
                .get(local_idx)
                .and_then(|&type_idx| self.types.get(type_idx as usize))
        }
    }

    /// Build a map from export name to function index.
    pub fn export_function_map(&self) -> HashMap<String, u32> {
        self.exports
            .iter()
            .filter(|e| e.kind == ExportKind::Function)
            .map(|e| (e.name.clone(), e.index))
            .collect()
    }
}

/// Parse a const expression.
fn parse_const_expr(name: &str, expr: wasmparser::ConstExpr<'_>) -> Result<ConstExpr, ComposeError> {
    let reader = expr.get_operators_reader();
    for op_result in reader {
        let op = op_result.map_err(|e| parse_error(name, e))?;
        match op {
            Operator::I32Const { value } => return Ok(ConstExpr::I32Const(value)),
            Operator::I64Const { value } => return Ok(ConstExpr::I64Const(value)),
            Operator::F32Const { value } => return Ok(ConstExpr::F32Const(f32::from_bits(value.bits()))),
            Operator::F64Const { value } => return Ok(ConstExpr::F64Const(f64::from_bits(value.bits()))),
            Operator::GlobalGet { global_index } => return Ok(ConstExpr::GlobalGet(global_index)),
            Operator::RefNull { hty } => {
                let kind = match hty {
                    wasmparser::HeapType::Abstract { ty, .. } => match ty {
                        wasmparser::AbstractHeapType::Extern => RefTypeKind::Extern,
                        _ => RefTypeKind::Func,
                    },
                    _ => RefTypeKind::Func,
                };
                return Ok(ConstExpr::RefNull(kind));
            }
            Operator::RefFunc { function_index } => return Ok(ConstExpr::RefFunc(function_index)),
            Operator::End => break,
            _ => continue,
        }
    }
    // Default
    Ok(ConstExpr::I32Const(0))
}

/// Convert a wasmparser operator to our stored format.
fn convert_operator(op: Operator<'_>) -> Option<StoredOperator> {
    Some(match op {
        // Control flow
        Operator::Unreachable => StoredOperator::Unreachable,
        Operator::Nop => StoredOperator::Nop,
        Operator::Block { blockty } => StoredOperator::Block(convert_block_type(blockty)),
        Operator::Loop { blockty } => StoredOperator::Loop(convert_block_type(blockty)),
        Operator::If { blockty } => StoredOperator::If(convert_block_type(blockty)),
        Operator::Else => StoredOperator::Else,
        Operator::End => StoredOperator::End,
        Operator::Br { relative_depth } => StoredOperator::Br(relative_depth),
        Operator::BrIf { relative_depth } => StoredOperator::BrIf(relative_depth),
        Operator::BrTable { targets } => StoredOperator::BrTable {
            targets: targets.targets().map(|t| t.unwrap()).collect(),
            default: targets.default(),
        },
        Operator::Return => StoredOperator::Return,
        Operator::Call { function_index } => StoredOperator::Call(function_index),
        Operator::CallIndirect { type_index, table_index, .. } => {
            StoredOperator::CallIndirect { type_index, table_index }
        }
        Operator::ReturnCall { function_index } => StoredOperator::ReturnCall(function_index),
        Operator::ReturnCallIndirect { type_index, table_index } => {
            StoredOperator::ReturnCallIndirect { type_index, table_index }
        }

        // Parametric
        Operator::Drop => StoredOperator::Drop,
        Operator::Select => StoredOperator::Select,
        Operator::TypedSelect { ty } => StoredOperator::TypedSelect(ty),

        // Variable access
        Operator::LocalGet { local_index } => StoredOperator::LocalGet(local_index),
        Operator::LocalSet { local_index } => StoredOperator::LocalSet(local_index),
        Operator::LocalTee { local_index } => StoredOperator::LocalTee(local_index),
        Operator::GlobalGet { global_index } => StoredOperator::GlobalGet(global_index),
        Operator::GlobalSet { global_index } => StoredOperator::GlobalSet(global_index),

        // Table operations
        Operator::TableGet { table } => StoredOperator::TableGet(table),
        Operator::TableSet { table } => StoredOperator::TableSet(table),
        Operator::TableGrow { table } => StoredOperator::TableGrow(table),
        Operator::TableSize { table } => StoredOperator::TableSize(table),
        Operator::TableFill { table } => StoredOperator::TableFill(table),
        Operator::TableCopy { dst_table, src_table } => {
            StoredOperator::TableCopy { dst: dst_table, src: src_table }
        }
        Operator::TableInit { elem_index, table } => {
            StoredOperator::TableInit { elem: elem_index, table }
        }
        Operator::ElemDrop { elem_index } => StoredOperator::ElemDrop(elem_index),

        // Memory load operations
        Operator::I32Load { memarg } => StoredOperator::I32Load(convert_memarg(memarg)),
        Operator::I64Load { memarg } => StoredOperator::I64Load(convert_memarg(memarg)),
        Operator::F32Load { memarg } => StoredOperator::F32Load(convert_memarg(memarg)),
        Operator::F64Load { memarg } => StoredOperator::F64Load(convert_memarg(memarg)),
        Operator::I32Load8S { memarg } => StoredOperator::I32Load8S(convert_memarg(memarg)),
        Operator::I32Load8U { memarg } => StoredOperator::I32Load8U(convert_memarg(memarg)),
        Operator::I32Load16S { memarg } => StoredOperator::I32Load16S(convert_memarg(memarg)),
        Operator::I32Load16U { memarg } => StoredOperator::I32Load16U(convert_memarg(memarg)),
        Operator::I64Load8S { memarg } => StoredOperator::I64Load8S(convert_memarg(memarg)),
        Operator::I64Load8U { memarg } => StoredOperator::I64Load8U(convert_memarg(memarg)),
        Operator::I64Load16S { memarg } => StoredOperator::I64Load16S(convert_memarg(memarg)),
        Operator::I64Load16U { memarg } => StoredOperator::I64Load16U(convert_memarg(memarg)),
        Operator::I64Load32S { memarg } => StoredOperator::I64Load32S(convert_memarg(memarg)),
        Operator::I64Load32U { memarg } => StoredOperator::I64Load32U(convert_memarg(memarg)),

        // Memory store operations
        Operator::I32Store { memarg } => StoredOperator::I32Store(convert_memarg(memarg)),
        Operator::I64Store { memarg } => StoredOperator::I64Store(convert_memarg(memarg)),
        Operator::F32Store { memarg } => StoredOperator::F32Store(convert_memarg(memarg)),
        Operator::F64Store { memarg } => StoredOperator::F64Store(convert_memarg(memarg)),
        Operator::I32Store8 { memarg } => StoredOperator::I32Store8(convert_memarg(memarg)),
        Operator::I32Store16 { memarg } => StoredOperator::I32Store16(convert_memarg(memarg)),
        Operator::I64Store8 { memarg } => StoredOperator::I64Store8(convert_memarg(memarg)),
        Operator::I64Store16 { memarg } => StoredOperator::I64Store16(convert_memarg(memarg)),
        Operator::I64Store32 { memarg } => StoredOperator::I64Store32(convert_memarg(memarg)),

        // Memory operations
        Operator::MemorySize { mem, .. } => StoredOperator::MemorySize(mem),
        Operator::MemoryGrow { mem, .. } => StoredOperator::MemoryGrow(mem),
        Operator::MemoryInit { data_index, mem } => {
            StoredOperator::MemoryInit { data: data_index, mem }
        }
        Operator::DataDrop { data_index } => StoredOperator::DataDrop(data_index),
        Operator::MemoryCopy { dst_mem, src_mem } => {
            StoredOperator::MemoryCopy { dst: dst_mem, src: src_mem }
        }
        Operator::MemoryFill { mem } => StoredOperator::MemoryFill(mem),

        // Constants
        Operator::I32Const { value } => StoredOperator::I32Const(value),
        Operator::I64Const { value } => StoredOperator::I64Const(value),
        Operator::F32Const { value } => StoredOperator::F32Const(f32::from_bits(value.bits())),
        Operator::F64Const { value } => StoredOperator::F64Const(f64::from_bits(value.bits())),

        // Comparison operators
        Operator::I32Eqz => StoredOperator::I32Eqz,
        Operator::I32Eq => StoredOperator::I32Eq,
        Operator::I32Ne => StoredOperator::I32Ne,
        Operator::I32LtS => StoredOperator::I32LtS,
        Operator::I32LtU => StoredOperator::I32LtU,
        Operator::I32GtS => StoredOperator::I32GtS,
        Operator::I32GtU => StoredOperator::I32GtU,
        Operator::I32LeS => StoredOperator::I32LeS,
        Operator::I32LeU => StoredOperator::I32LeU,
        Operator::I32GeS => StoredOperator::I32GeS,
        Operator::I32GeU => StoredOperator::I32GeU,

        Operator::I64Eqz => StoredOperator::I64Eqz,
        Operator::I64Eq => StoredOperator::I64Eq,
        Operator::I64Ne => StoredOperator::I64Ne,
        Operator::I64LtS => StoredOperator::I64LtS,
        Operator::I64LtU => StoredOperator::I64LtU,
        Operator::I64GtS => StoredOperator::I64GtS,
        Operator::I64GtU => StoredOperator::I64GtU,
        Operator::I64LeS => StoredOperator::I64LeS,
        Operator::I64LeU => StoredOperator::I64LeU,
        Operator::I64GeS => StoredOperator::I64GeS,
        Operator::I64GeU => StoredOperator::I64GeU,

        Operator::F32Eq => StoredOperator::F32Eq,
        Operator::F32Ne => StoredOperator::F32Ne,
        Operator::F32Lt => StoredOperator::F32Lt,
        Operator::F32Gt => StoredOperator::F32Gt,
        Operator::F32Le => StoredOperator::F32Le,
        Operator::F32Ge => StoredOperator::F32Ge,

        Operator::F64Eq => StoredOperator::F64Eq,
        Operator::F64Ne => StoredOperator::F64Ne,
        Operator::F64Lt => StoredOperator::F64Lt,
        Operator::F64Gt => StoredOperator::F64Gt,
        Operator::F64Le => StoredOperator::F64Le,
        Operator::F64Ge => StoredOperator::F64Ge,

        // Numeric operators - i32
        Operator::I32Clz => StoredOperator::I32Clz,
        Operator::I32Ctz => StoredOperator::I32Ctz,
        Operator::I32Popcnt => StoredOperator::I32Popcnt,
        Operator::I32Add => StoredOperator::I32Add,
        Operator::I32Sub => StoredOperator::I32Sub,
        Operator::I32Mul => StoredOperator::I32Mul,
        Operator::I32DivS => StoredOperator::I32DivS,
        Operator::I32DivU => StoredOperator::I32DivU,
        Operator::I32RemS => StoredOperator::I32RemS,
        Operator::I32RemU => StoredOperator::I32RemU,
        Operator::I32And => StoredOperator::I32And,
        Operator::I32Or => StoredOperator::I32Or,
        Operator::I32Xor => StoredOperator::I32Xor,
        Operator::I32Shl => StoredOperator::I32Shl,
        Operator::I32ShrS => StoredOperator::I32ShrS,
        Operator::I32ShrU => StoredOperator::I32ShrU,
        Operator::I32Rotl => StoredOperator::I32Rotl,
        Operator::I32Rotr => StoredOperator::I32Rotr,

        // Numeric operators - i64
        Operator::I64Clz => StoredOperator::I64Clz,
        Operator::I64Ctz => StoredOperator::I64Ctz,
        Operator::I64Popcnt => StoredOperator::I64Popcnt,
        Operator::I64Add => StoredOperator::I64Add,
        Operator::I64Sub => StoredOperator::I64Sub,
        Operator::I64Mul => StoredOperator::I64Mul,
        Operator::I64DivS => StoredOperator::I64DivS,
        Operator::I64DivU => StoredOperator::I64DivU,
        Operator::I64RemS => StoredOperator::I64RemS,
        Operator::I64RemU => StoredOperator::I64RemU,
        Operator::I64And => StoredOperator::I64And,
        Operator::I64Or => StoredOperator::I64Or,
        Operator::I64Xor => StoredOperator::I64Xor,
        Operator::I64Shl => StoredOperator::I64Shl,
        Operator::I64ShrS => StoredOperator::I64ShrS,
        Operator::I64ShrU => StoredOperator::I64ShrU,
        Operator::I64Rotl => StoredOperator::I64Rotl,
        Operator::I64Rotr => StoredOperator::I64Rotr,

        // Numeric operators - f32
        Operator::F32Abs => StoredOperator::F32Abs,
        Operator::F32Neg => StoredOperator::F32Neg,
        Operator::F32Ceil => StoredOperator::F32Ceil,
        Operator::F32Floor => StoredOperator::F32Floor,
        Operator::F32Trunc => StoredOperator::F32Trunc,
        Operator::F32Nearest => StoredOperator::F32Nearest,
        Operator::F32Sqrt => StoredOperator::F32Sqrt,
        Operator::F32Add => StoredOperator::F32Add,
        Operator::F32Sub => StoredOperator::F32Sub,
        Operator::F32Mul => StoredOperator::F32Mul,
        Operator::F32Div => StoredOperator::F32Div,
        Operator::F32Min => StoredOperator::F32Min,
        Operator::F32Max => StoredOperator::F32Max,
        Operator::F32Copysign => StoredOperator::F32Copysign,

        // Numeric operators - f64
        Operator::F64Abs => StoredOperator::F64Abs,
        Operator::F64Neg => StoredOperator::F64Neg,
        Operator::F64Ceil => StoredOperator::F64Ceil,
        Operator::F64Floor => StoredOperator::F64Floor,
        Operator::F64Trunc => StoredOperator::F64Trunc,
        Operator::F64Nearest => StoredOperator::F64Nearest,
        Operator::F64Sqrt => StoredOperator::F64Sqrt,
        Operator::F64Add => StoredOperator::F64Add,
        Operator::F64Sub => StoredOperator::F64Sub,
        Operator::F64Mul => StoredOperator::F64Mul,
        Operator::F64Div => StoredOperator::F64Div,
        Operator::F64Min => StoredOperator::F64Min,
        Operator::F64Max => StoredOperator::F64Max,
        Operator::F64Copysign => StoredOperator::F64Copysign,

        // Conversions
        Operator::I32WrapI64 => StoredOperator::I32WrapI64,
        Operator::I32TruncF32S => StoredOperator::I32TruncF32S,
        Operator::I32TruncF32U => StoredOperator::I32TruncF32U,
        Operator::I32TruncF64S => StoredOperator::I32TruncF64S,
        Operator::I32TruncF64U => StoredOperator::I32TruncF64U,
        Operator::I64ExtendI32S => StoredOperator::I64ExtendI32S,
        Operator::I64ExtendI32U => StoredOperator::I64ExtendI32U,
        Operator::I64TruncF32S => StoredOperator::I64TruncF32S,
        Operator::I64TruncF32U => StoredOperator::I64TruncF32U,
        Operator::I64TruncF64S => StoredOperator::I64TruncF64S,
        Operator::I64TruncF64U => StoredOperator::I64TruncF64U,
        Operator::F32ConvertI32S => StoredOperator::F32ConvertI32S,
        Operator::F32ConvertI32U => StoredOperator::F32ConvertI32U,
        Operator::F32ConvertI64S => StoredOperator::F32ConvertI64S,
        Operator::F32ConvertI64U => StoredOperator::F32ConvertI64U,
        Operator::F32DemoteF64 => StoredOperator::F32DemoteF64,
        Operator::F64ConvertI32S => StoredOperator::F64ConvertI32S,
        Operator::F64ConvertI32U => StoredOperator::F64ConvertI32U,
        Operator::F64ConvertI64S => StoredOperator::F64ConvertI64S,
        Operator::F64ConvertI64U => StoredOperator::F64ConvertI64U,
        Operator::F64PromoteF32 => StoredOperator::F64PromoteF32,
        Operator::I32ReinterpretF32 => StoredOperator::I32ReinterpretF32,
        Operator::I64ReinterpretF64 => StoredOperator::I64ReinterpretF64,
        Operator::F32ReinterpretI32 => StoredOperator::F32ReinterpretI32,
        Operator::F64ReinterpretI64 => StoredOperator::F64ReinterpretI64,

        // Sign extension
        Operator::I32Extend8S => StoredOperator::I32Extend8S,
        Operator::I32Extend16S => StoredOperator::I32Extend16S,
        Operator::I64Extend8S => StoredOperator::I64Extend8S,
        Operator::I64Extend16S => StoredOperator::I64Extend16S,
        Operator::I64Extend32S => StoredOperator::I64Extend32S,

        // Saturating truncation
        Operator::I32TruncSatF32S => StoredOperator::I32TruncSatF32S,
        Operator::I32TruncSatF32U => StoredOperator::I32TruncSatF32U,
        Operator::I32TruncSatF64S => StoredOperator::I32TruncSatF64S,
        Operator::I32TruncSatF64U => StoredOperator::I32TruncSatF64U,
        Operator::I64TruncSatF32S => StoredOperator::I64TruncSatF32S,
        Operator::I64TruncSatF32U => StoredOperator::I64TruncSatF32U,
        Operator::I64TruncSatF64S => StoredOperator::I64TruncSatF64S,
        Operator::I64TruncSatF64U => StoredOperator::I64TruncSatF64U,

        // Reference types
        Operator::RefNull { hty } => {
            let kind = match hty {
                wasmparser::HeapType::Abstract { ty, .. } => match ty {
                    wasmparser::AbstractHeapType::Extern => RefTypeKind::Extern,
                    _ => RefTypeKind::Func,
                },
                _ => RefTypeKind::Func,
            };
            StoredOperator::RefNull(kind)
        }
        Operator::RefIsNull => StoredOperator::RefIsNull,
        Operator::RefFunc { function_index } => StoredOperator::RefFunc(function_index),

        // Unknown/unsupported operators - skip them
        _ => return None,
    })
}

fn convert_block_type(bt: wasmparser::BlockType) -> BlockType {
    match bt {
        wasmparser::BlockType::Empty => BlockType::Empty,
        wasmparser::BlockType::Type(ty) => BlockType::Type(ty),
        wasmparser::BlockType::FuncType(idx) => BlockType::FuncType(idx),
    }
}

fn convert_memarg(memarg: wasmparser::MemArg) -> MemArg {
    MemArg {
        offset: memarg.offset,
        align: memarg.align as u32,
        memory: memarg.memory,
    }
}

fn parse_error(module: &str, e: BinaryReaderError) -> ComposeError {
    ComposeError::ParseError {
        module: module.to_string(),
        message: e.to_string(),
    }
}
