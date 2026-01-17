//! Composite: A component runtime with extended WIT support
//!
//! This runtime extends the WebAssembly Component Model with support for
//! recursive data types, enabling natural representation of tree structures
//! like ASTs, S-expressions, and other recursive data.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │           Composite Runtime             │
//! │                                         │
//! │  wit_plus  - Extended WIT parsing       │
//! │  abi       - Type encoding/decoding     │
//! │  runtime   - Component instantiation    │
//! │                                         │
//! ├─────────────────────────────────────────┤
//! │         WASM Execution (wasmi)          │
//! └─────────────────────────────────────────┘
//! ```
//!
//! ## Extended WIT Types
//!
//! WIT+ allows recursive types by default:
//!
//! ```wit
//! variant sexpr {
//!     sym(string),
//!     num(s64),
//!     lst(list<sexpr>),
//! }
//! ```

pub mod abi;
pub mod runtime;
pub mod wit_plus;

pub use abi::{decode, encode};
pub use runtime::{
    validate_instance_implements_interface, CompiledModule, Ctx, DefaultHostProvider,
    HostFunctionProvider, HostLinkerBuilder, Instance, InterfaceBuilder, InterfaceError,
    LinkerError, Runtime,
};
pub use wit_plus::{Interface, TypeDef};
