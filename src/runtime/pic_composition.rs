//! Shared-memory (PIC) package composition — N packages, one linear memory, one
//! allocator.
//!
//! Unlike [`CompositionBuilder`](super::CompositionBuilder) (which gives each
//! package its own memory and copies encoded bytes across a cross-memory bridge),
//! here every package is a PIC side module at a disjoint base in ONE shared
//! `Memory`, on ONE shared in-wasm allocator. A consumer's import is satisfied by
//! a thin shim that calls the provider's export **in the same memory** (zero
//! copy) and re-labels the ABI status so the consumer frees the provider's result
//! buffer. See `docs/pic-composition.md` (level 2).

use std::collections::{HashMap, HashSet};

use wasmtime::{
    Caller, Engine, Instance, Linker, Memory, MemoryType, Module, Ref, RefType, Store, Table,
    TableType,
};

use crate::abi::{decode, encode, Value};
use crate::runtime::{
    assert_pic_module, const_g, resolve_got_data_end, var_g, werr, RuntimeError, PACK_ALLOC_WASM,
};

// Per-package region: data grows up from `base`, stack grows down from
// `base + REGION_SIZE`. The next package's `base` is this one's stack top — they
// grow apart from that boundary, so they never collide as long as each package's
// (static data + peak stack) stays under REGION_SIZE.
const REGION_SIZE: i32 = 0x8_0000; // 512 KiB per package
const TABLE_STRIDE: i32 = 128; // shared-table slots reserved per package
const FIRST_BASE: i32 = 0x1_0000; // first package base (addr 0 stays a guard)
const ALLOC_BSS: i32 = 0x8_000; // room for the allocator's control struct
const HEAP_SIZE: i32 = 0x40_0000; // 4 MiB shared heap
const PAGE: i32 = 0x1_0000; // 64 KiB wasm page

struct PkgSpec {
    name: String,
    wasm: Vec<u8>,
}

struct Wiring {
    consumer: String,
    import_module: String,
    import_function: String,
    provider: String,
    provider_export: String,
}

/// Builds a [`PicComposition`]: add PIC packages, wire consumer imports to
/// provider exports, then `build()`.
pub struct PicCompositionBuilder<'e> {
    engine: &'e Engine,
    packages: Vec<PkgSpec>,
    wirings: Vec<Wiring>,
}

impl<'e> PicCompositionBuilder<'e> {
    pub fn new(engine: &'e Engine) -> Self {
        Self {
            engine,
            packages: Vec::new(),
            wirings: Vec::new(),
        }
    }

    /// Add a PIC package (built with the 0.8.x PIC recipe + `packr-guest`'s `pic`
    /// feature).
    pub fn add_package(mut self, name: impl Into<String>, wasm: Vec<u8>) -> Self {
        self.packages.push(PkgSpec {
            name: name.into(),
            wasm,
        });
        self
    }

    /// Wire `consumer`'s import `import_module::import_function` to `provider`'s
    /// `provider_export`.
    pub fn wire(
        mut self,
        consumer: impl Into<String>,
        import_module: impl Into<String>,
        import_function: impl Into<String>,
        provider: impl Into<String>,
        provider_export: impl Into<String>,
    ) -> Self {
        self.wirings.push(Wiring {
            consumer: consumer.into(),
            import_module: import_module.into(),
            import_function: import_function.into(),
            provider: provider.into(),
            provider_export: provider_export.into(),
        });
        self
    }

    pub fn build(self) -> Result<PicComposition, RuntimeError> {
        // Dependency-order the packages: a consumer is instantiated after every
        // provider it wires from (DAG only, no cycles).
        let order = topo_order(&self.packages, &self.wirings)?;
        let n = order.len() as i32;

        // Memory layout: packages, then the allocator + heap above them.
        let alloc_base = FIRST_BASE + n * REGION_SIZE;
        let heap_base = alloc_base + ALLOC_BSS;
        let heap_end = heap_base + HEAP_SIZE;
        let init_pages = (heap_end / PAGE + 1) as u32;

        let mut store = Store::new(self.engine, ());
        let mem = Memory::new(&mut store, MemoryType::new(init_pages, None)).map_err(werr)?;
        let table = Table::new(
            &mut store,
            TableType::new(RefType::FUNCREF, (n * TABLE_STRIDE) as u32 + 64, None),
            Ref::Func(None),
        )
        .map_err(werr)?;

        // Shared in-wasm allocator, serving every package's heap.
        let alloc_module = Module::new(self.engine, PACK_ALLOC_WASM).map_err(werr)?;
        let a_membase = const_g(&mut store, alloc_base);
        let a_tablebase = const_g(&mut store, 0);
        let a_heap_base = var_g(&mut store, heap_base);
        let a_heap_end = var_g(&mut store, heap_end);
        let mut al = Linker::new(self.engine);
        al.define(&store, "env", "memory", mem).map_err(werr)?;
        al.define(&store, "env", "__memory_base", a_membase)
            .map_err(werr)?;
        al.define(&store, "env", "__table_base", a_tablebase)
            .map_err(werr)?;
        al.define(&store, "GOT.mem", "__heap_base", a_heap_base)
            .map_err(werr)?;
        al.define(&store, "GOT.mem", "__heap_end", a_heap_end)
            .map_err(werr)?;
        let alloc_inst = al.instantiate(&mut store, &alloc_module).map_err(werr)?;
        let alloc_fn = alloc_inst
            .get_export(&mut store, "alloc")
            .ok_or_else(|| RuntimeError::WasmError("allocator missing `alloc`".into()))?;
        let dealloc_fn = alloc_inst
            .get_export(&mut store, "dealloc")
            .ok_or_else(|| RuntimeError::WasmError("allocator missing `dealloc`".into()))?;

        let modules: HashMap<&str, Module> = self
            .packages
            .iter()
            .map(|p| {
                Module::new(self.engine, &p.wasm)
                    .map_err(werr)
                    .map(|m| (p.name.as_str(), m))
            })
            .collect::<Result<_, _>>()?;

        let mut instances: HashMap<String, Instance> = HashMap::new();

        for (i, name) in order.iter().enumerate() {
            let module = &modules[name.as_str()];
            assert_pic_module(module)?;

            let base = FIRST_BASE + (i as i32) * REGION_SIZE;
            let stack_top = base + REGION_SIZE;
            let table_base = (i as i32) * TABLE_STRIDE;

            let p_data_end = var_g(&mut store, base);
            let p_sp = var_g(&mut store, stack_top);
            let p_membase = const_g(&mut store, base);
            let p_tablebase = const_g(&mut store, table_base);
            let mut pl = Linker::new(self.engine);
            pl.define(&store, "env", "memory", mem).map_err(werr)?;
            pl.define(&store, "env", "__indirect_function_table", table)
                .map_err(werr)?;
            pl.define(&store, "env", "__stack_pointer", p_sp)
                .map_err(werr)?;
            pl.define(&store, "env", "__memory_base", p_membase)
                .map_err(werr)?;
            pl.define(&store, "env", "__table_base", p_tablebase)
                .map_err(werr)?;
            pl.define(&store, "GOT.mem", "__data_end", p_data_end)
                .map_err(werr)?;
            pl.define(&store, "pack:alloc", "alloc", alloc_fn.clone())
                .map_err(werr)?;
            pl.define(&store, "pack:alloc", "dealloc", dealloc_fn.clone())
                .map_err(werr)?;

            // Cross-package imports: wire each of this consumer's imports to the
            // provider's export via a shim (below).
            for w in self.wirings.iter().filter(|w| &w.consumer == name) {
                let provider_inst = *instances.get(&w.provider).ok_or_else(|| {
                    RuntimeError::WasmError(format!(
                        "wire: provider `{}` not instantiated before consumer `{}`",
                        w.provider, name
                    ))
                })?;
                let provider_export = provider_inst
                    .get_func(&mut store, &w.provider_export)
                    .ok_or_else(|| {
                        RuntimeError::FunctionNotFound(format!(
                            "`{}` has no export `{}`",
                            w.provider, w.provider_export
                        ))
                    })?;
                let target = format!("{}::{}", w.provider, w.provider_export);
                pl.func_wrap(
                    &w.import_module,
                    &w.import_function,
                    move |mut caller: Caller<'_, ()>,
                          in_ptr: i32,
                          in_len: i32,
                          out_ptr_ptr: i32,
                          out_len_ptr: i32|
                          -> Result<i32, wasmtime::Error> {
                        // Call the provider's export in the SAME memory (no copy).
                        // It guest-allocates its result buffer from the shared heap
                        // and writes ptr/len into the caller's slots.
                        let typed = provider_export.typed::<(i32, i32, i32, i32), i32>(&caller)?;
                        let status =
                            typed.call(&mut caller, (in_ptr, in_len, out_ptr_ptr, out_len_ptr))?;
                        if status != 0 {
                            return Ok(status); // propagate the provider's error
                        }
                        // Re-label as guest-owned (1) so the consumer's
                        // __import_impl frees the provider's result buffer — same
                        // shared allocator, so the free is unambiguous.
                        let _ = &target;
                        Ok(1)
                    },
                )
                .map_err(werr)?;
            }

            let inst = pl.instantiate(&mut store, module).map_err(werr)?;
            resolve_got_data_end(&mut store, &inst, p_data_end)?;
            if let Ok(relocs) =
                inst.get_typed_func::<(), ()>(&mut store, "__wasm_apply_data_relocs")
            {
                relocs.call(&mut store, ()).map_err(werr)?;
            }
            if let Ok(ctors) = inst.get_typed_func::<(), ()>(&mut store, "__wasm_call_ctors") {
                ctors.call(&mut store, ()).map_err(werr)?;
            }
            instances.insert(name.clone(), inst);
        }

        Ok(PicComposition {
            store,
            memory: mem,
            instances,
        })
    }
}

/// A built shared-memory composition. Call any package's exports; cross-package
/// imports run in the same memory.
pub struct PicComposition {
    store: Store<()>,
    memory: Memory,
    instances: HashMap<String, Instance>,
}

impl PicComposition {
    /// Call `package::function(input)` through the shared memory + allocator.
    pub fn call(
        &mut self,
        package: &str,
        function: &str,
        input: &Value,
    ) -> Result<Value, RuntimeError> {
        let inst = *self
            .instances
            .get(package)
            .ok_or_else(|| RuntimeError::ModuleNotFound(format!("package `{package}`")))?;

        let bytes = encode(input).map_err(|e| RuntimeError::AbiError(e.to_string()))?;
        let pack_alloc = inst
            .get_typed_func::<i32, i32>(&mut self.store, "__pack_alloc")
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;
        let pack_free = inst.get_typed_func::<(i32, i32), ()>(&mut self.store, "__pack_free");

        let in_ptr = pack_alloc
            .call(&mut self.store, bytes.len() as i32)
            .map_err(werr)?;
        self.memory
            .write(&mut self.store, in_ptr as usize, &bytes)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        let slots = pack_alloc.call(&mut self.store, 8).map_err(werr)?;

        let func = inst
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut self.store, function)
            .map_err(|e| RuntimeError::FunctionNotFound(e.to_string()))?;
        let status = func
            .call(
                &mut self.store,
                (in_ptr, bytes.len() as i32, slots, slots + 4),
            )
            .map_err(werr)?;

        let out_ptr = self.read_i32(slots)?;
        let out_len = self.read_i32(slots + 4)?;
        let mut out = vec![0u8; out_len as usize];
        self.memory
            .read(&self.store, out_ptr as usize, &mut out)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;

        // Free the buffers this call owns (input, slots, and the result once
        // decoded), so a long-lived composition doesn't leak per call.
        if let Ok(free) = pack_free {
            let _ = free.call(&mut self.store, (in_ptr, bytes.len() as i32));
            let _ = free.call(&mut self.store, (slots, 8));
            let _ = free.call(&mut self.store, (out_ptr, out_len));
        }

        if status != 0 {
            return Err(RuntimeError::WasmError(format!(
                "guest error in {package}::{function}: {}",
                String::from_utf8_lossy(&out)
            )));
        }
        decode(&out).map_err(|e| RuntimeError::AbiError(e.to_string()))
    }

    pub fn packages(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    /// Current shared linear-memory size in bytes (grows via `memory.grow`, never
    /// shrinks) — for observing that cross-package calls don't leak.
    pub fn memory_size(&self) -> usize {
        self.memory.data_size(&self.store)
    }

    fn read_i32(&self, at: i32) -> Result<i32, RuntimeError> {
        let mut b = [0u8; 4];
        self.memory
            .read(&self.store, at as usize, &mut b)
            .map_err(|e| RuntimeError::MemoryError(e.to_string()))?;
        Ok(i32::from_le_bytes(b))
    }
}

/// Topological order (providers before consumers). DAG only.
fn topo_order(packages: &[PkgSpec], wirings: &[Wiring]) -> Result<Vec<String>, RuntimeError> {
    let names: HashSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let mut deps: HashMap<&str, HashSet<&str>> = packages
        .iter()
        .map(|p| (p.name.as_str(), HashSet::new()))
        .collect();
    for w in wirings {
        if !names.contains(w.provider.as_str()) {
            return Err(RuntimeError::ModuleNotFound(format!(
                "wire references unknown provider `{}`",
                w.provider
            )));
        }
        deps.get_mut(w.consumer.as_str())
            .ok_or_else(|| {
                RuntimeError::ModuleNotFound(format!(
                    "wire references unknown consumer `{}`",
                    w.consumer
                ))
            })?
            .insert(w.provider.as_str());
    }

    let mut order: Vec<String> = Vec::new();
    let mut done: HashSet<&str> = HashSet::new();
    while order.len() < packages.len() {
        let mut progressed = false;
        for p in packages {
            if done.contains(p.name.as_str()) {
                continue;
            }
            if deps[p.name.as_str()].iter().all(|d| done.contains(d)) {
                order.push(p.name.clone());
                done.insert(p.name.as_str());
                progressed = true;
            }
        }
        if !progressed {
            return Err(RuntimeError::WasmError(
                "circular package dependency in composition (DAG only)".into(),
            ));
        }
    }
    Ok(order)
}
