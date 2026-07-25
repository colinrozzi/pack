//! Static verification of a composite / self-contained actor wasm.
//!
//! A composed (or plain self-contained) actor's only legitimate imports are host
//! functions — everything else (cross-component `mesh.*`, `math.*`, …) must be
//! internalized by composition. `--host-only` asserts exactly that, so a build
//! pipeline can gate on "this composite is deployable" in one shot instead of
//! grep-parsing `wasm-tools print`.

use anyhow::Result;
use wasmparser::{Parser, Payload};

/// The host import namespace for theater actors. Every import a self-contained or
/// composed actor legitimately has is a host function under this prefix.
pub const HOST_MODULE_PREFIX: &str = "theater:simple/";

/// Return every `(module, name)` import whose module is NOT under `host_prefix`.
///
/// An empty result means the module is **host-only**: every import is a host
/// function, so nothing cross-component was left unsatisfied. A non-empty result
/// lists exactly the imports that make it non-deployable (an unsatisfied link, or
/// an unexpected dependency).
pub fn non_host_imports(wasm: &[u8], host_prefix: &str) -> Result<Vec<(String, String)>> {
    let mut offenders = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::ImportSection(reader) = payload? {
            for import in reader {
                let import = import?;
                if !import.module.starts_with(host_prefix) {
                    offenders.push((import.module.to_string(), import.name.to_string()));
                }
            }
        }
    }
    Ok(offenders)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny module importing one host fn and one cross-component fn, and
    /// confirm only the non-host one is flagged.
    #[test]
    fn flags_only_non_host_imports() {
        let mut m = walrus::Module::default();
        let ty = m.types.add(&[], &[]);
        m.add_import_func("theater:simple/runtime", "log", ty);
        m.add_import_func("mesh", "submit", ty);
        let wasm = m.emit_wasm();

        let offenders = non_host_imports(&wasm, HOST_MODULE_PREFIX).unwrap();
        assert_eq!(offenders, vec![("mesh".to_string(), "submit".to_string())]);
    }

    /// A module whose imports are all under the host prefix is host-only.
    #[test]
    fn all_host_imports_is_host_only() {
        let mut m = walrus::Module::default();
        let ty = m.types.add(&[], &[]);
        m.add_import_func("theater:simple/runtime", "log", ty);
        m.add_import_func("theater:simple/message-server-host", "request", ty);
        let wasm = m.emit_wasm();

        assert!(non_host_imports(&wasm, HOST_MODULE_PREFIX)
            .unwrap()
            .is_empty());
    }
}
