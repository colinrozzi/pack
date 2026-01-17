//! Interface enforcement - validate WASM modules implement WIT interfaces

use crate::wit_plus::{Function, Interface};
use thiserror::Error;
use wasmi::{Instance, Store};

/// Errors from interface validation
#[derive(Error, Debug)]
pub enum InterfaceError {
    #[error("Missing function '{name}' required by interface")]
    MissingFunction { name: String },

    #[error("Function '{name}' has wrong signature: expected {expected}, got {actual}")]
    SignatureMismatch {
        name: String,
        expected: String,
        actual: String,
    },

    #[error("Missing memory export 'memory'")]
    MissingMemory,
}

/// The expected WASM signature for Graph ABI functions
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExpectedSignature {
    /// (i32, i32) -> i64: pointer/length in, packed pointer/length out
    GraphAbi,
    /// () -> () for functions with no params and no results
    NoArgsNoResults,
}

impl ExpectedSignature {
    fn description(&self) -> &'static str {
        match self {
            ExpectedSignature::GraphAbi => "(i32, i32) -> i64",
            ExpectedSignature::NoArgsNoResults => "() -> ()",
        }
    }
}

/// Determine expected WASM signature for a WIT function
fn expected_signature_for(func: &Function) -> ExpectedSignature {
    // Graph ABI convention: all functions with params or results use (i32, i32) -> i64
    // The pointer/length convention handles all types uniformly
    if func.params.is_empty() && func.results.is_empty() {
        ExpectedSignature::NoArgsNoResults
    } else {
        ExpectedSignature::GraphAbi
    }
}

/// Validate that a WASM instance implements all functions required by an interface
///
/// Checks:
/// - All functions declared in `export` blocks exist
/// - All top-level functions (not in import/export blocks) exist
/// - Functions have correct WASM-level signatures for the Graph ABI
/// - Memory export exists (required for value passing)
///
/// # Example
///
/// ```ignore
/// let interface = parse_interface(r#"
///     interface api {
///         process: func(input: string) -> string;
///         export api {
///             process: func(input: string) -> string;
///         }
///     }
/// "#)?;
///
/// let module = runtime.load_module(&wasm_bytes)?;
/// let instance = module.instantiate()?;
///
/// validate_instance_implements_interface(&instance.store, &instance.wasm_instance, &interface)?;
/// ```
pub fn validate_instance_implements_interface<T>(
    store: &Store<T>,
    instance: &Instance,
    interface: &Interface,
) -> Result<(), InterfaceError> {
    // Check memory exists
    if instance.get_memory(store, "memory").is_none() {
        return Err(InterfaceError::MissingMemory);
    }

    // Collect all functions that should be exported
    let mut required_functions: Vec<&Function> = Vec::new();

    // Top-level functions (not in import/export blocks)
    required_functions.extend(interface.functions.iter());

    // Functions in export blocks
    for export_block in &interface.exports {
        required_functions.extend(export_block.functions.iter());
    }

    // Check each required function
    for func in required_functions {
        check_function_export(store, instance, func)?;
    }

    Ok(())
}

fn check_function_export<T>(
    store: &Store<T>,
    instance: &Instance,
    func: &Function,
) -> Result<(), InterfaceError> {
    let expected_sig = expected_signature_for(func);

    match expected_sig {
        ExpectedSignature::GraphAbi => {
            // Try to get the function with (i32, i32) -> i64 signature
            match instance.get_typed_func::<(i32, i32), i64>(store, &func.name) {
                Ok(_) => Ok(()),
                Err(_) => {
                    // Check if function exists at all with wrong signature
                    if instance.get_export(store, &func.name).is_some() {
                        Err(InterfaceError::SignatureMismatch {
                            name: func.name.clone(),
                            expected: expected_sig.description().to_string(),
                            actual: "different signature".to_string(),
                        })
                    } else {
                        Err(InterfaceError::MissingFunction {
                            name: func.name.clone(),
                        })
                    }
                }
            }
        }
        ExpectedSignature::NoArgsNoResults => {
            match instance.get_typed_func::<(), ()>(store, &func.name) {
                Ok(_) => Ok(()),
                Err(_) => {
                    if instance.get_export(store, &func.name).is_some() {
                        Err(InterfaceError::SignatureMismatch {
                            name: func.name.clone(),
                            expected: expected_sig.description().to_string(),
                            actual: "different signature".to_string(),
                        })
                    } else {
                        Err(InterfaceError::MissingFunction {
                            name: func.name.clone(),
                        })
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wit_plus::Type;

    #[test]
    fn expected_signature_graph_abi_for_params() {
        let func = Function {
            name: "process".to_string(),
            params: vec![("x".to_string(), Type::S64)],
            results: vec![Type::S64],
        };
        assert_eq!(expected_signature_for(&func), ExpectedSignature::GraphAbi);
    }

    #[test]
    fn expected_signature_no_args_for_empty() {
        let func = Function {
            name: "init".to_string(),
            params: vec![],
            results: vec![],
        };
        assert_eq!(
            expected_signature_for(&func),
            ExpectedSignature::NoArgsNoResults
        );
    }
}
