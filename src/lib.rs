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
//! Standard WIT types work as expected. Additionally, recursive types
//! are supported with the `rec` keyword:
//!
//! ```wit
//! rec variant sexpr {
//!     sym(string),
//!     num(s64),
//!     lst(list<sexpr>),
//! }
//! ```

pub mod abi;
pub mod runtime;
pub mod wit_plus;

pub use abi::{decode, encode};
pub use runtime::Runtime;
pub use wit_plus::{Interface, TypeDef};
