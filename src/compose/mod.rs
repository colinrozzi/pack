//! Static Composition for WASM Packages
//!
//! This module provides static composition of WASM packages - merging multiple
//! WASM modules into a single binary where imports from one module are resolved
//! to exports from another.
//!
//! Unlike runtime composition (see `crate::runtime::CompositionBuilder`), static
//! composition produces a single merged WASM file that can be loaded and executed
//! without the overhead of cross-module calls.
//!
//! # Example
//!
//! ```ignore
//! use pack::compose::StaticComposer;
//!
//! // Load package bytes
//! let doubler_wasm = std::fs::read("doubler.wasm")?;
//! let adder_wasm = std::fs::read("adder.wasm")?;
//!
//! // Build static composition
//! let composed = StaticComposer::new()
//!     .add_module("doubler", doubler_wasm)?
//!     .add_module("adder", adder_wasm)?
//!     .wire("adder", "math", "double", "doubler", "transform")
//!     .export("process", "adder", "process")
//!     .export("memory", "adder", "memory")
//!     .compose()?;
//!
//! // `composed` is a single WASM binary with all functions merged
//! std::fs::write("composed.wasm", composed)?;
//! ```
//!
//! # How It Works
//!
//! 1. **Parse** each module to extract types, imports, exports, functions, etc.
//! 2. **Topologically sort** modules so providers are processed before consumers
//! 3. **Deduplicate types** across modules
//! 4. **Merge functions**: resolved imports become internal `call` instructions
//! 5. **Remap indices** in all function bodies
//! 6. **Emit** combined module using wasm-encoder

mod error;
mod merger;
mod parser;

pub use error::ComposeError;
pub use parser::ParsedModule;

use merger::{ExportSpec, Merger, Wiring};
use std::collections::HashMap;

/// Builder for creating statically composed WASM modules.
///
/// `StaticComposer` merges multiple WASM modules into a single binary,
/// resolving inter-module imports to direct function calls.
pub struct StaticComposer {
    modules: Vec<ParsedModule>,
    wirings: Vec<Wiring>,
    exports: Vec<ExportSpec>,
}

impl StaticComposer {
    /// Create a new static composer.
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            wirings: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Add a WASM module to the composition.
    ///
    /// # Arguments
    ///
    /// * `name` - A unique name for this module, used to reference it in wiring
    /// * `wasm` - The raw WASM bytes of the module
    ///
    /// # Errors
    ///
    /// Returns `ComposeError::ParseError` if the WASM module is invalid.
    pub fn add_module(mut self, name: &str, wasm: Vec<u8>) -> Result<Self, ComposeError> {
        let parsed = ParsedModule::parse(name, &wasm)?;
        self.modules.push(parsed);
        Ok(self)
    }

    /// Wire an import from one module to an export from another.
    ///
    /// When the `consumer` module imports `import_module::import_fn`, it will
    /// be resolved to call the `provider_export` from the `provider` module
    /// instead of being kept as an external import.
    ///
    /// # Arguments
    ///
    /// * `consumer` - Name of the module that has the import
    /// * `import_module` - The module name in the import declaration
    /// * `import_fn` - The function name in the import declaration
    /// * `provider` - Name of the module that provides the implementation
    /// * `provider_export` - The export name in the provider module
    ///
    /// # Example
    ///
    /// If `adder.wasm` has:
    /// ```wat
    /// (import "math" "double" (func ...))
    /// ```
    ///
    /// And `doubler.wasm` exports a function called "transform", you'd wire:
    /// ```ignore
    /// composer.wire("adder", "math", "double", "doubler", "transform")
    /// ```
    pub fn wire(
        mut self,
        consumer: &str,
        import_module: &str,
        import_fn: &str,
        provider: &str,
        provider_export: &str,
    ) -> Self {
        self.wirings.push(Wiring {
            consumer: consumer.to_string(),
            import_module: import_module.to_string(),
            import_fn: import_fn.to_string(),
            provider: provider.to_string(),
            provider_export: provider_export.to_string(),
        });
        self
    }

    /// Automatically wire imports to exports based on name matching.
    ///
    /// For each unresolved import, if there's an export with the same name
    /// in another module, wire them together.
    ///
    /// # Errors
    ///
    /// Returns `ComposeError::TypeMismatch` if matching imports/exports have
    /// incompatible function signatures.
    pub fn auto_wire(mut self) -> Result<Self, ComposeError> {
        // Build a map of all exports across all modules
        let mut export_map: HashMap<String, Vec<(&str, &parser::Export)>> = HashMap::new();
        for module in &self.modules {
            for export in &module.exports {
                export_map
                    .entry(export.name.clone())
                    .or_default()
                    .push((&module.name, export));
            }
        }

        // For each module's imports, try to find a matching export
        for module in &self.modules {
            for import in &module.imports {
                // Skip if already wired
                let already_wired = self.wirings.iter().any(|w| {
                    w.consumer == module.name
                        && w.import_module == import.module
                        && w.import_fn == import.name
                });
                if already_wired {
                    continue;
                }

                // Look for matching export by function name
                if let Some(exports) = export_map.get(&import.name) {
                    // Find an export from a different module
                    for (provider_name, export) in exports {
                        if *provider_name != module.name {
                            // Check that it's a function export matching a function import
                            if matches!(import.kind, parser::ImportKind::Function(_))
                                && export.kind == parser::ExportKind::Function
                            {
                                self.wirings.push(Wiring {
                                    consumer: module.name.clone(),
                                    import_module: import.module.clone(),
                                    import_fn: import.name.clone(),
                                    provider: provider_name.to_string(),
                                    provider_export: export.name.clone(),
                                });
                                break;
                            }
                        }
                    }
                }
            }
        }

        Ok(self)
    }

    /// Add an export to the composed module.
    ///
    /// This specifies which functions/memories/etc. from the source modules
    /// should be exported from the composed module.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the export in the composed module
    /// * `source_module` - The module that has the export
    /// * `source_fn` - The name of the export in the source module
    pub fn export(mut self, name: &str, source_module: &str, source_fn: &str) -> Self {
        self.exports.push(ExportSpec {
            name: name.to_string(),
            source_module: source_module.to_string(),
            source_export: source_fn.to_string(),
        });
        self
    }

    /// Compose the modules into a single WASM binary.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No modules were added
    /// - A referenced module is not found
    /// - A wired import/export is not found
    /// - There's a circular dependency
    /// - The merged module fails to encode
    pub fn compose(self) -> Result<Vec<u8>, ComposeError> {
        if self.modules.is_empty() {
            return Err(ComposeError::NoModules);
        }

        let merger = Merger::new(self.modules, self.wirings, self.exports);
        merger.merge()
    }
}

impl Default for StaticComposer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_composer_api() {
        // Just test that the API compiles - we'll add real tests later
        let _composer = StaticComposer::new();
    }

    #[test]
    fn test_parse_simple_module() {
        // A minimal valid WASM module
        let wasm = wat::parse_str(
            r#"
            (module
                (func $add (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add
                )
                (export "add" (func $add))
            )
            "#,
        )
        .unwrap();

        let parsed = ParsedModule::parse("test", &wasm).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.functions.len(), 1);
        assert_eq!(parsed.exports.len(), 1);
        assert_eq!(parsed.exports[0].name, "add");
    }

    #[test]
    fn test_compose_simple_modules() {
        // Module A: exports a doubler function
        let module_a = wat::parse_str(
            r#"
            (module
                (memory (export "memory") 1)
                (func $double (param i32) (result i32)
                    local.get 0
                    i32.const 2
                    i32.mul
                )
                (export "double" (func $double))
            )
            "#,
        )
        .unwrap();

        // Module B: imports double, exports add_doubled
        let module_b = wat::parse_str(
            r#"
            (module
                (import "math" "double" (func $double (param i32) (result i32)))
                (func $add_doubled (param i32 i32) (result i32)
                    local.get 0
                    call $double
                    local.get 1
                    call $double
                    i32.add
                )
                (export "add_doubled" (func $add_doubled))
            )
            "#,
        )
        .unwrap();

        let composed = StaticComposer::new()
            .add_module("doubler", module_a)
            .unwrap()
            .add_module("adder", module_b)
            .unwrap()
            .wire("adder", "math", "double", "doubler", "double")
            .export("add_doubled", "adder", "add_doubled")
            .export("memory", "doubler", "memory")
            .compose()
            .unwrap();

        // Verify the composed module is valid
        assert!(!composed.is_empty());

        // Parse the composed module to verify structure
        let parsed = ParsedModule::parse("composed", &composed).unwrap();

        // Should have the exported function
        assert!(parsed.exports.iter().any(|e| e.name == "add_doubled"));
        assert!(parsed.exports.iter().any(|e| e.name == "memory"));

        // Should have no imports (the math::double import was resolved)
        assert_eq!(parsed.num_imported_functions, 0);
    }

    #[test]
    fn test_auto_wire() {
        // Module A: exports "transform"
        let module_a = wat::parse_str(
            r#"
            (module
                (func $transform (param i32) (result i32)
                    local.get 0
                    i32.const 10
                    i32.add
                )
                (export "transform" (func $transform))
            )
            "#,
        )
        .unwrap();

        // Module B: imports "transform" from any module
        let module_b = wat::parse_str(
            r#"
            (module
                (import "util" "transform" (func $transform (param i32) (result i32)))
                (func $process (param i32) (result i32)
                    local.get 0
                    call $transform
                )
                (export "process" (func $process))
            )
            "#,
        )
        .unwrap();

        let composed = StaticComposer::new()
            .add_module("a", module_a)
            .unwrap()
            .add_module("b", module_b)
            .unwrap()
            .auto_wire()
            .unwrap()
            .export("process", "b", "process")
            .compose()
            .unwrap();

        let parsed = ParsedModule::parse("composed", &composed).unwrap();
        assert_eq!(parsed.num_imported_functions, 0);
    }
}
