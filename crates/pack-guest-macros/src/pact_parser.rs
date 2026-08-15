//! Minimal Pact parser for proc macro use.
//!
//! This is a simplified version of the Pact parser that runs at compile time
//! within proc macros.

use proc_macro2::Span;
use std::collections::HashMap;

/// A parsed interface path like "namespace:package/interface"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InterfacePath {
    pub namespace: Option<String>,
    pub package: Option<String>,
    pub interface: String,
}

impl InterfacePath {
    /// Parse an interface path from a string like "theater:simple/actor"
    pub fn parse(s: &str) -> Option<Self> {
        // Format: namespace:package/interface or package/interface or just interface
        if let Some((ns_pkg, iface)) = s.rsplit_once('/') {
            if let Some((ns, pkg)) = ns_pkg.split_once(':') {
                Some(InterfacePath {
                    namespace: Some(ns.to_string()),
                    package: Some(pkg.to_string()),
                    interface: iface.to_string(),
                })
            } else {
                Some(InterfacePath {
                    namespace: None,
                    package: Some(ns_pkg.to_string()),
                    interface: iface.to_string(),
                })
            }
        } else {
            Some(InterfacePath {
                namespace: None,
                package: None,
                interface: s.to_string(),
            })
        }
    }

    /// Convert to a string representation
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        match (&self.namespace, &self.package) {
            (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, self.interface),
            (None, Some(pkg)) => format!("{}/{}", pkg, self.interface),
            _ => self.interface.clone(),
        }
    }
}

/// A full function path like "theater:simple/actor.init"
#[derive(Debug, Clone)]
pub struct FunctionPath {
    pub interface: InterfacePath,
    pub function: String,
}

impl FunctionPath {
    /// Parse a function path from a string like "theater:simple/actor.init"
    pub fn parse(s: &str) -> Option<Self> {
        // Format: interface-path.function or interface-path#function
        let (iface_str, func) = if let Some((iface, func)) = s.rsplit_once('.') {
            (iface, func)
        } else if let Some((iface, func)) = s.rsplit_once('#') {
            (iface, func)
        } else {
            return None;
        };

        Some(FunctionPath {
            interface: InterfacePath::parse(iface_str)?,
            function: func.to_string(),
        })
    }

    /// Get the canonical export name (using '.' separator)
    pub fn export_name(&self) -> String {
        format!("{}.{}", self.interface.to_string(), self.function)
    }
}

/// Registry of all parsed Pact content
#[derive(Debug, Clone, Default)]
pub struct PactRegistry {
    /// Current package declaration (namespace:package)
    pub current_package: Option<(String, String)>,
    /// All interfaces indexed by their full path
    pub interfaces: HashMap<String, Interface>,
    /// All worlds
    pub worlds: Vec<World>,
    /// All top-level type definitions (for the current package)
    pub types: Vec<TypeDef>,
}

impl PactRegistry {
    /// Look up a function by its full path
    pub fn find_function(&self, path: &FunctionPath) -> Option<&Function> {
        let iface_key = path.interface.to_string();
        if let Some(iface) = self.interfaces.get(&iface_key) {
            return iface.functions.iter().find(|f| f.name == path.function);
        }

        // Also check world exports
        for world in &self.worlds {
            for export in &world.exports {
                match export {
                    WorldItem::Function(f) if f.name == path.function => return Some(f),
                    WorldItem::InlineInterface { name, functions } => {
                        // Check if this matches the interface name
                        if *name == path.interface.interface || path.interface.to_string() == *name
                        {
                            if let Some(f) = functions.iter().find(|f| f.name == path.function) {
                                return Some(f);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }

    /// Check if a function exists (by simple name) in any export
    #[allow(dead_code)]
    pub fn has_export_function(&self, func_name: &str) -> bool {
        for world in &self.worlds {
            for export in &world.exports {
                match export {
                    WorldItem::Function(f) if f.name == func_name => return true,
                    WorldItem::InlineInterface { functions, .. } => {
                        if functions.iter().any(|f| f.name == func_name) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Check top-level interfaces
        for iface in self.interfaces.values() {
            if iface.functions.iter().any(|f| f.name == func_name) {
                return true;
            }
        }

        false
    }

    /// Get all available export names for error messages
    pub fn available_exports(&self) -> Vec<String> {
        let mut names = Vec::new();

        for world in &self.worlds {
            for export in &world.exports {
                match export {
                    WorldItem::Function(f) => names.push(f.name.clone()),
                    WorldItem::InlineInterface { name, functions } => {
                        for f in functions {
                            names.push(format!("{}.{}", name, f.name));
                        }
                    }
                    WorldItem::InterfacePath {
                        namespace,
                        package,
                        interface,
                    } => {
                        let path = match (namespace, package) {
                            (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                            (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                            _ => interface.clone(),
                        };
                        names.push(format!("<{}>", path));
                    }
                }
            }
        }

        // Add functions from top-level interfaces
        for (path, iface) in &self.interfaces {
            for f in &iface.functions {
                names.push(format!("{}.{}", path, f.name));
            }
        }

        names
    }

    /// Get all available import names for error messages
    pub fn available_imports(&self) -> Vec<String> {
        let mut names = Vec::new();

        for world in &self.worlds {
            for import in &world.imports {
                match import {
                    WorldItem::Function(f) => names.push(f.name.clone()),
                    WorldItem::InlineInterface { name, functions } => {
                        for f in functions {
                            names.push(format!("{}.{}", name, f.name));
                        }
                    }
                    WorldItem::InterfacePath {
                        namespace,
                        package,
                        interface,
                    } => {
                        let path = match (namespace, package) {
                            (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                            (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                            _ => interface.clone(),
                        };
                        // Check if this interface exists in the registry
                        if let Some(iface) = self.interfaces.get(&path) {
                            for f in &iface.functions {
                                names.push(format!("{}.{}", path, f.name));
                            }
                        } else {
                            names.push(format!("<{}>", path));
                        }
                    }
                }
            }
        }

        // Add functions from top-level interfaces (could be imports)
        for (path, iface) in &self.interfaces {
            for f in &iface.functions {
                names.push(format!("{}.{}", path, f.name));
            }
        }

        names
    }

    /// Find an import function by its path
    pub fn find_import_function(&self, path: &FunctionPath) -> Option<&Function> {
        let iface_key = path.interface.to_string();

        // Check interfaces registry
        if let Some(iface) = self.interfaces.get(&iface_key) {
            return iface.functions.iter().find(|f| f.name == path.function);
        }

        // Check world imports
        for world in &self.worlds {
            for import in &world.imports {
                match import {
                    WorldItem::Function(f) if f.name == path.function => return Some(f),
                    WorldItem::InlineInterface { name, functions } => {
                        if *name == path.interface.interface || path.interface.to_string() == *name
                        {
                            if let Some(f) = functions.iter().find(|f| f.name == path.function) {
                                return Some(f);
                            }
                        }
                    }
                    WorldItem::InterfacePath {
                        namespace,
                        package,
                        interface,
                    } => {
                        let import_path = match (namespace, package) {
                            (Some(ns), Some(pkg)) => format!("{}:{}/{}", ns, pkg, interface),
                            (None, Some(pkg)) => format!("{}/{}", pkg, interface),
                            _ => interface.clone(),
                        };
                        if import_path == iface_key {
                            // The interface is imported but we need to look it up
                            if let Some(iface) = self.interfaces.get(&import_path) {
                                return iface.functions.iter().find(|f| f.name == path.function);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        None
    }
}

/// A Pact type reference
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    // Primitives
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,

    // Compound
    List(Box<Type>),
    Option(Box<Type>),
    Result {
        ok: Option<Box<Type>>,
        err: Option<Box<Type>>,
    },
    Tuple(Vec<Type>),

    // `map<K, V>` — front-end sugar that lowers to `BTreeMap<K, V>` in Rust and
    // erases to `list<tuple<K, V>>` on the wire / in metadata.
    Map {
        key: Box<Type>,
        value: Box<Type>,
    },

    // Named reference (to another type). Also used for a reference to an
    // in-scope generic type parameter (e.g. `a` inside `record pair<a, b>`);
    // both lower to a PascalCased Rust identifier, so codegen need not
    // distinguish them.
    Named(String),

    // Generic type application: a named generic type applied to type
    // arguments, e.g. `pair<u32, string>` or the recursive `tree<t>`.
    App {
        name: String,
        args: Vec<Type>,
    },

    // Self-reference within a type definition (for recursion)
    SelfRef,
}

/// A type definition
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// type foo = bar (optionally generic: `type foo<a> = ...`)
    Alias {
        name: String,
        type_params: Vec<String>,
        ty: Type,
    },

    /// record foo { field: type, ... } (optionally generic: `record foo<a, b>`)
    Record {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
    },

    /// variant foo { case(payload), ... } (optionally generic: `variant foo<t>`)
    Variant {
        name: String,
        type_params: Vec<String>,
        cases: Vec<VariantCase>,
    },

    /// enum foo { a, b, c }
    Enum { name: String, cases: Vec<String> },

    /// flags foo { a, b, c }
    Flags { name: String, flags: Vec<String> },
}

impl Type {
    /// Substitute in-scope generic parameters (matched by name against `env`)
    /// with concrete types. This is the core of generic instantiation on the
    /// guest side (see [`TypeDef::instantiate`]).
    pub fn substitute(&self, env: &std::collections::HashMap<String, Type>) -> Type {
        match self {
            Type::Named(name) => env.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::List(inner) => Type::List(Box::new(inner.substitute(env))),
            Type::Option(inner) => Type::Option(Box::new(inner.substitute(env))),
            Type::Result { ok, err } => Type::Result {
                ok: ok.as_ref().map(|t| Box::new(t.substitute(env))),
                err: err.as_ref().map(|t| Box::new(t.substitute(env))),
            },
            Type::Tuple(items) => Type::Tuple(items.iter().map(|t| t.substitute(env)).collect()),
            Type::Map { key, value } => Type::Map {
                key: Box::new(key.substitute(env)),
                value: Box::new(value.substitute(env)),
            },
            Type::App { name, args } => Type::App {
                name: name.clone(),
                args: args.iter().map(|t| t.substitute(env)).collect(),
            },
            _ => self.clone(),
        }
    }
}

impl TypeDef {
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Alias { name, .. } => name,
            TypeDef::Record { name, .. } => name,
            TypeDef::Variant { name, .. } => name,
            TypeDef::Enum { name, .. } => name,
            TypeDef::Flags { name, .. } => name,
        }
    }

    /// Generic type parameters (empty for non-generic defs and enum/flags).
    pub fn type_params(&self) -> &[String] {
        match self {
            TypeDef::Alias { type_params, .. }
            | TypeDef::Record { type_params, .. }
            | TypeDef::Variant { type_params, .. } => type_params,
            TypeDef::Enum { .. } | TypeDef::Flags { .. } => &[],
        }
    }

    /// Instantiate a generic definition by binding its type parameters to
    /// concrete `args`, producing a monomorphic definition with every
    /// parameter reference substituted away. Callers check arity first.
    pub fn instantiate(&self, args: &[Type]) -> TypeDef {
        let env: std::collections::HashMap<String, Type> = self
            .type_params()
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        match self {
            TypeDef::Alias { name, ty, .. } => TypeDef::Alias {
                name: name.clone(),
                type_params: Vec::new(),
                ty: ty.substitute(&env),
            },
            TypeDef::Record { name, fields, .. } => TypeDef::Record {
                name: name.clone(),
                type_params: Vec::new(),
                fields: fields
                    .iter()
                    .map(|(n, t)| (n.clone(), t.substitute(&env)))
                    .collect(),
            },
            TypeDef::Variant { name, cases, .. } => TypeDef::Variant {
                name: name.clone(),
                type_params: Vec::new(),
                cases: cases
                    .iter()
                    .map(|c| VariantCase {
                        name: c.name.clone(),
                        payload: c.payload.as_ref().map(|t| t.substitute(&env)),
                    })
                    .collect(),
            },
            TypeDef::Enum { .. } | TypeDef::Flags { .. } => self.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantCase {
    pub name: String,
    pub payload: Option<Type>,
}

/// A function signature
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub results: Vec<Type>,
}

/// A parsed Pact interface
#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub functions: Vec<Function>,
}

/// A world item (import or export)
#[derive(Debug, Clone)]
pub enum WorldItem {
    /// A function: `name: func(...) -> ...`
    Function(Function),

    /// An interface path: `wasi:cli/stdin`
    InterfacePath {
        namespace: Option<String>,
        package: Option<String>,
        interface: String,
    },

    /// Inline interface: `name { func... }`
    InlineInterface {
        name: String,
        functions: Vec<Function>,
    },
}

/// A parsed Pact world
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct World {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub imports: Vec<WorldItem>,
    pub exports: Vec<WorldItem>,
}

/// Parse error
#[derive(Debug)]
#[allow(dead_code)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl ParseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: Span::call_site(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ============================================================================
// Tokenizer
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Ident(String),
    Symbol(char),
    Eof,
}

struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().peekable(),
        }
    }

    fn tokenize(&mut self) -> Result<Vec<Token>, ParseError> {
        let mut tokens = Vec::new();

        while let Some(&ch) = self.chars.peek() {
            if ch.is_whitespace() {
                self.chars.next();
                continue;
            }

            // Comments
            if ch == '/' {
                self.chars.next();
                if matches!(self.chars.peek(), Some('/')) {
                    // Line comment
                    for c in self.chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                }
                if matches!(self.chars.peek(), Some('*')) {
                    // Block comment
                    self.chars.next();
                    while let Some(c) = self.chars.next() {
                        if c == '*' && matches!(self.chars.peek(), Some('/')) {
                            self.chars.next();
                            break;
                        }
                    }
                    continue;
                }
                tokens.push(Token::Symbol('/'));
                continue;
            }

            // Identifiers
            if ch.is_ascii_alphabetic() || ch == '_' {
                let mut ident = String::new();
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        ident.push(c);
                        self.chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(ident));
                continue;
            }

            // Symbols
            if matches!(
                ch,
                '{' | '}' | '(' | ')' | '<' | '>' | ':' | ',' | '=' | ';' | '-' | '.' | '@' | '*'
            ) {
                tokens.push(Token::Symbol(ch));
                self.chars.next();
                continue;
            }

            return Err(ParseError::new(format!("unexpected character: {}", ch)));
        }

        tokens.push(Token::Eof);
        Ok(tokens)
    }
}

// ============================================================================
// Parser
// ============================================================================

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn peek_n(&self, n: usize) -> &Token {
        self.tokens.get(self.pos + n).unwrap_or(&Token::Eof)
    }

    fn next(&mut self) -> Token {
        let tok = self.peek().clone();
        if !matches!(tok, Token::Eof) {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn accept_symbol(&mut self, expected: char) -> bool {
        if matches!(self.peek(), Token::Symbol(c) if *c == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect_symbol(&mut self, expected: char) -> Result<(), ParseError> {
        match self.next() {
            Token::Symbol(c) if c == expected => Ok(()),
            other => Err(ParseError::new(format!(
                "expected '{}', got {:?}",
                expected, other
            ))),
        }
    }

    pub(crate) fn accept_ident(&mut self, expected: &str) -> bool {
        if matches!(self.peek(), Token::Ident(s) if s == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.next() {
            Token::Ident(s) => Ok(s),
            other => Err(ParseError::new(format!(
                "expected identifier, got {:?}",
                other
            ))),
        }
    }

    pub(crate) fn is_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    pub(crate) fn peek_is_symbol(&self, expected: char) -> bool {
        matches!(self.peek(), Token::Symbol(c) if *c == expected)
    }

    /// Peek n tokens ahead and check if it's a specific identifier.
    pub(crate) fn peek_n_is_ident(&self, n: usize, expected: &str) -> bool {
        matches!(self.peek_n(n), Token::Ident(s) if s == expected)
    }

    pub(crate) fn peek_n_is_symbol(&self, n: usize, expected: char) -> bool {
        matches!(self.peek_n(n), Token::Symbol(c) if *c == expected)
    }
}

// ============================================================================
// Crate-internal helpers
// ============================================================================

pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut lexer = Lexer::new(src);
    lexer.tokenize()
}

pub(crate) fn make_parser(tokens: Vec<Token>) -> Parser {
    Parser::new(tokens)
}

// ============================================================================
// Public parsing functions
// ============================================================================

/// Parse a Pact world definition
pub fn parse_world(src: &str) -> Result<World, ParseError> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);

    // Parse optional type definitions before the world
    let mut types = Vec::new();
    while !parser.is_eof() {
        if matches!(parser.peek(), Token::Ident(s) if s == "world") {
            break;
        }
        if let Some(typedef) = try_parse_typedef(&mut parser)? {
            types.push(typedef);
        } else {
            break;
        }
    }

    // Parse world keyword
    if !parser.accept_ident("world") {
        return Err(ParseError::new("expected 'world' keyword"));
    }

    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;

    let mut imports = Vec::new();
    let mut exports = Vec::new();

    while !parser.is_eof() && !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        // Check for type definitions inside world
        if let Some(typedef) = try_parse_typedef(&mut parser)? {
            types.push(typedef);
            continue;
        }

        let keyword = parser.expect_ident()?;
        match keyword.as_str() {
            "import" => imports.push(parse_world_item(&mut parser)?),
            "export" => exports.push(parse_world_item(&mut parser)?),
            _ => {
                return Err(ParseError::new(format!(
                    "expected 'import' or 'export', got '{}'",
                    keyword
                )))
            }
        }
    }

    parser.expect_symbol('}')?;

    Ok(World {
        name,
        types,
        imports,
        exports,
    })
}

/// Parse Pact content and return a complete registry
pub fn parse_pact(src: &str) -> Result<PactRegistry, ParseError> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);

    let mut registry = PactRegistry::default();

    while !parser.is_eof() {
        // Skip semicolons
        if parser.accept_symbol(';') {
            continue;
        }

        let keyword = match parser.peek() {
            Token::Ident(s) => s.clone(),
            Token::Eof => break,
            other => return Err(ParseError::new(format!("unexpected token: {:?}", other))),
        };

        match keyword.as_str() {
            "package" => {
                parser.next();
                // Parse package declaration: namespace:name or just name
                let first = parser.expect_ident()?;
                if parser.accept_symbol(':') {
                    let second = parser.expect_ident()?;
                    registry.current_package = Some((first, second));
                } else {
                    // No namespace, just package name
                    registry.current_package = Some((String::new(), first));
                }
                parser.accept_symbol(';');
            }
            "interface" => {
                parser.next();
                let iface = parse_interface(&mut parser)?;

                // Build the full interface path
                let path = if let Some((ns, pkg)) = &registry.current_package {
                    if ns.is_empty() {
                        format!("{}/{}", pkg, iface.name)
                    } else {
                        format!("{}:{}/{}", ns, pkg, iface.name)
                    }
                } else {
                    iface.name.clone()
                };

                registry.interfaces.insert(path, iface);
            }
            "world" => {
                parser.next();
                let world = parse_world_body(&mut parser)?;
                registry.worlds.push(world);
            }
            // Type definitions at top level
            "type" | "record" | "variant" | "enum" | "flags" => {
                if let Some(typedef) = try_parse_typedef(&mut parser)? {
                    registry.types.push(typedef);
                }
            }
            // Use statements (skip for now)
            "use" => {
                parser.next();
                // Skip until semicolon
                while !parser.is_eof() && !parser.accept_symbol(';') {
                    parser.next();
                }
            }
            _ => {
                // Skip unknown tokens
                parser.next();
            }
        }
    }

    Ok(registry)
}

/// Parse an interface definition
fn parse_interface(parser: &mut Parser) -> Result<Interface, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;

    let mut types = Vec::new();
    let mut functions = Vec::new();

    while !parser.is_eof() && !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        // Try to parse a type definition
        if let Some(typedef) = try_parse_typedef(parser)? {
            types.push(typedef);
            continue;
        }

        // Try to parse use statement
        if parser.accept_ident("use") {
            // Skip until semicolon or end of line
            while !parser.is_eof() && !parser.accept_symbol(';') {
                parser.next();
            }
            continue;
        }

        // Otherwise, try to parse a function
        // Format: name: func(...) -> ...
        if let (Token::Ident(_func_name), Token::Symbol(':')) =
            (parser.peek().clone(), parser.peek_n(1).clone())
        {
            let func_name = parser.expect_ident()?;
            parser.expect_symbol(':')?;

            if parser.accept_ident("func") {
                let func = parse_func_signature(parser, func_name)?;
                functions.push(func);
                parser.accept_symbol(';');
                continue;
            }
        }

        // Skip unknown content
        parser.next();
    }

    parser.expect_symbol('}')?;

    Ok(Interface {
        name,
        types,
        functions,
    })
}

/// Parse just the body of a world (after 'world' keyword has been consumed)
fn parse_world_body(parser: &mut Parser) -> Result<World, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;

    let mut types = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    while !parser.is_eof() && !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        // Check for type definitions inside world
        if let Some(typedef) = try_parse_typedef(parser)? {
            types.push(typedef);
            continue;
        }

        // Check for use statement
        if parser.accept_ident("use") {
            // Skip until semicolon
            while !parser.is_eof() && !parser.accept_symbol(';') {
                parser.next();
            }
            continue;
        }

        let keyword = parser.expect_ident()?;
        match keyword.as_str() {
            "import" => imports.push(parse_world_item(parser)?),
            "export" => exports.push(parse_world_item(parser)?),
            _ => {
                return Err(ParseError::new(format!(
                    "expected 'import' or 'export', got '{}'",
                    keyword
                )))
            }
        }
    }

    parser.expect_symbol('}')?;

    Ok(World {
        name,
        types,
        imports,
        exports,
    })
}

/// Try to parse a type definition from the current parser position.
/// Returns None if the next token doesn't start a type definition.
pub fn try_parse_typedef_public(parser: &mut Parser) -> Result<Option<TypeDef>, ParseError> {
    try_parse_typedef(parser)
}

/// Parse an optional generic parameter list following a type name: `<a, b>`.
/// Returns an empty vec if no `<` follows.
fn parse_type_param_list(parser: &mut Parser) -> Result<Vec<String>, ParseError> {
    let mut params = Vec::new();
    if !parser.accept_symbol('<') {
        return Ok(params);
    }
    loop {
        if parser.accept_symbol('>') {
            break;
        }
        params.push(parser.expect_ident()?);
        if parser.accept_symbol('>') {
            break;
        }
        parser.expect_symbol(',')?;
    }
    Ok(params)
}

/// Parse `<t, u, ...>` as a list of type arguments (leading `<` required).
fn parse_type_arg_list(parser: &mut Parser) -> Result<Vec<Type>, ParseError> {
    parser.expect_symbol('<')?;
    let mut args = Vec::new();
    loop {
        if parser.accept_symbol('>') {
            break;
        }
        args.push(parse_type(parser)?);
        if parser.accept_symbol('>') {
            break;
        }
        parser.expect_symbol(',')?;
    }
    Ok(args)
}

fn try_parse_typedef(parser: &mut Parser) -> Result<Option<TypeDef>, ParseError> {
    let keyword = match parser.peek() {
        Token::Ident(s) => s.clone(),
        _ => return Ok(None),
    };

    match keyword.as_str() {
        "type" => {
            parser.next();
            let name = parser.expect_ident()?;
            let type_params = parse_type_param_list(parser)?;
            parser.expect_symbol('=')?;
            let ty = parse_type(parser)?;
            Ok(Some(TypeDef::Alias {
                name,
                type_params,
                ty,
            }))
        }
        "record" => {
            parser.next();
            let name = parser.expect_ident()?;
            let type_params = parse_type_param_list(parser)?;
            parser.expect_symbol('{')?;
            let mut fields = Vec::new();
            while !parser.accept_symbol('}') {
                let field_name = parser.expect_ident()?;
                parser.expect_symbol(':')?;
                let field_type = parse_type(parser)?;
                fields.push((field_name, field_type));
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Record {
                name,
                type_params,
                fields,
            }))
        }
        "variant" => {
            parser.next();
            let name = parser.expect_ident()?;
            let type_params = parse_type_param_list(parser)?;
            parser.expect_symbol('{')?;
            let mut cases = Vec::new();
            while !parser.accept_symbol('}') {
                let case_name = parser.expect_ident()?;
                let payload = if parser.accept_symbol('(') {
                    let ty = parse_type(parser)?;
                    parser.expect_symbol(')')?;
                    Some(ty)
                } else {
                    None
                };
                cases.push(VariantCase {
                    name: case_name,
                    payload,
                });
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Variant {
                name,
                type_params,
                cases,
            }))
        }
        "enum" => {
            parser.next();
            let name = parser.expect_ident()?;
            parser.expect_symbol('{')?;
            let mut cases = Vec::new();
            while !parser.accept_symbol('}') {
                cases.push(parser.expect_ident()?);
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Enum { name, cases }))
        }
        "flags" => {
            parser.next();
            let name = parser.expect_ident()?;
            parser.expect_symbol('{')?;
            let mut flags = Vec::new();
            while !parser.accept_symbol('}') {
                flags.push(parser.expect_ident()?);
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Flags { name, flags }))
        }
        _ => Ok(None),
    }
}

fn parse_world_item(parser: &mut Parser) -> Result<WorldItem, ParseError> {
    let first = parser.expect_ident()?;

    // Check for colon - could be function or namespace
    if parser.accept_symbol(':') {
        // Check if next is 'func' keyword
        if parser.accept_ident("func") {
            let func = parse_func_signature(parser, first)?;
            return Ok(WorldItem::Function(func));
        }

        // Otherwise it's namespace:package/interface
        let package = parser.expect_ident()?;
        parser.expect_symbol('/')?;
        let interface = parser.expect_ident()?;

        return Ok(WorldItem::InterfacePath {
            namespace: Some(first),
            package: Some(package),
            interface,
        });
    }

    // Check for inline interface
    if parser.accept_symbol('{') {
        let functions = parse_function_block(parser)?;
        parser.expect_symbol('}')?;
        return Ok(WorldItem::InlineInterface {
            name: first,
            functions,
        });
    }

    // Check for package/interface (no namespace)
    if parser.accept_symbol('/') {
        let interface = parser.expect_ident()?;
        return Ok(WorldItem::InterfacePath {
            namespace: None,
            package: Some(first),
            interface,
        });
    }

    // Simple interface reference
    Ok(WorldItem::InterfacePath {
        namespace: None,
        package: None,
        interface: first,
    })
}

fn parse_function_block(parser: &mut Parser) -> Result<Vec<Function>, ParseError> {
    let mut functions = Vec::new();

    while !parser.is_eof() && !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        // Try name: func(...) pattern
        if let (Token::Ident(name), Token::Symbol(':'), Token::Ident(func_kw)) = (
            parser.peek().clone(),
            parser.peek_n(1).clone(),
            parser.peek_n(2).clone(),
        ) {
            if func_kw == "func" {
                parser.next(); // name
                parser.next(); // :
                parser.next(); // func
                functions.push(parse_func_signature(parser, name)?);
                continue;
            }
        }

        // Try bare 'func' keyword
        if parser.accept_ident("func") {
            let name = parser.expect_ident()?;
            functions.push(parse_func_signature(parser, name)?);
            continue;
        }

        break;
    }

    Ok(functions)
}

pub(crate) fn parse_func_signature(
    parser: &mut Parser,
    name: String,
) -> Result<Function, ParseError> {
    parser.expect_symbol('(')?;
    let params = parse_params(parser)?;
    parser.expect_symbol(')')?;

    let results = if parser.accept_symbol('-') {
        parser.expect_symbol('>')?;
        parse_results(parser)?
    } else {
        Vec::new()
    };

    Ok(Function {
        name,
        params,
        results,
    })
}

fn parse_params(parser: &mut Parser) -> Result<Vec<(String, Type)>, ParseError> {
    let mut params = Vec::new();

    if matches!(parser.peek(), Token::Symbol(')')) {
        return Ok(params);
    }

    loop {
        let name = parser.expect_ident()?;
        parser.expect_symbol(':')?;
        let ty = parse_type(parser)?;
        params.push((name, ty));

        if matches!(parser.peek(), Token::Symbol(')')) {
            break;
        }
        parser.expect_symbol(',')?;
    }

    Ok(params)
}

fn parse_results(parser: &mut Parser) -> Result<Vec<Type>, ParseError> {
    // Handle _ for no results
    if parser.accept_ident("_") {
        return Ok(Vec::new());
    }

    // Handle tuple results
    if parser.accept_symbol('(') {
        let mut results = Vec::new();
        if parser.accept_symbol(')') {
            return Ok(results);
        }
        loop {
            results.push(parse_type(parser)?);
            if parser.accept_symbol(')') {
                break;
            }
            parser.expect_symbol(',')?;
        }
        return Ok(results);
    }

    // Single result
    Ok(vec![parse_type(parser)?])
}

pub(crate) fn parse_type(parser: &mut Parser) -> Result<Type, ParseError> {
    let ident = parser.expect_ident()?;

    match ident.as_str() {
        "bool" => Ok(Type::Bool),
        "u8" => Ok(Type::U8),
        "u16" => Ok(Type::U16),
        "u32" => Ok(Type::U32),
        "u64" => Ok(Type::U64),
        "s8" => Ok(Type::S8),
        "s16" => Ok(Type::S16),
        "s32" => Ok(Type::S32),
        "s64" => Ok(Type::S64),
        "f32" => Ok(Type::F32),
        "f64" => Ok(Type::F64),
        "char" => Ok(Type::Char),
        "string" => Ok(Type::String),
        "self" => Ok(Type::SelfRef),
        "list" => {
            parser.expect_symbol('<')?;
            let inner = parse_type(parser)?;
            parser.expect_symbol('>')?;
            Ok(Type::List(Box::new(inner)))
        }
        "option" => {
            parser.expect_symbol('<')?;
            let inner = parse_type(parser)?;
            parser.expect_symbol('>')?;
            Ok(Type::Option(Box::new(inner)))
        }
        "tuple" => {
            parser.expect_symbol('<')?;
            let mut items = Vec::new();
            loop {
                if parser.accept_symbol('>') {
                    break;
                }
                items.push(parse_type(parser)?);
                if parser.accept_symbol('>') {
                    break;
                }
                parser.expect_symbol(',')?;
            }
            Ok(Type::Tuple(items))
        }
        "result" => {
            parser.expect_symbol('<')?;
            let ok = parse_optional_type(parser)?;
            parser.expect_symbol(',')?;
            let err = parse_optional_type(parser)?;
            parser.expect_symbol('>')?;
            Ok(Type::Result {
                ok: ok.map(Box::new),
                err: err.map(Box::new),
            })
        }
        "map" => {
            parser.expect_symbol('<')?;
            let key = parse_type(parser)?;
            parser.expect_symbol(',')?;
            let value = parse_type(parser)?;
            parser.expect_symbol('>')?;
            Ok(Type::Map {
                key: Box::new(key),
                value: Box::new(value),
            })
        }
        _ => {
            // Generic type application `name<...>`, or a bare named reference
            // (which also covers in-scope type-parameter references).
            if matches!(parser.peek(), Token::Symbol('<')) {
                let args = parse_type_arg_list(parser)?;
                Ok(Type::App { name: ident, args })
            } else {
                Ok(Type::Named(ident))
            }
        }
    }
}

fn parse_optional_type(parser: &mut Parser) -> Result<Option<Type>, ParseError> {
    if parser.accept_ident("_") {
        Ok(None)
    } else {
        Ok(Some(parse_type(parser)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_world() {
        let src = r#"
            world my-component {
                import log: func(msg: string)
                export run: func() -> string
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.name, "my-component");
        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.exports.len(), 1);
    }

    #[test]
    fn test_parse_world_with_types() {
        let src = r#"
            variant sexpr {
                sym(string),
                num(s64),
                cons(list<sexpr>),
                nil,
            }

            world evaluator {
                export eval: func(expr: sexpr) -> sexpr
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.name, "evaluator");
        assert_eq!(world.types.len(), 1);
        assert_eq!(world.exports.len(), 1);

        // Check the variant
        match &world.types[0] {
            TypeDef::Variant { name, cases, .. } => {
                assert_eq!(name, "sexpr");
                assert_eq!(cases.len(), 4);
            }
            _ => panic!("expected variant"),
        }
    }

    #[test]
    fn test_parse_record() {
        let src = r#"
            record point {
                x: s32,
                y: s32,
            }

            world geo {
                export translate: func(p: point, dx: s32, dy: s32) -> point
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.types.len(), 1);

        match &world.types[0] {
            TypeDef::Record { name, fields, .. } => {
                assert_eq!(name, "point");
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("expected record"),
        }
    }
}

#[cfg(test)]
mod generic_tests {
    use super::*;

    #[test]
    fn parses_generic_typedefs_and_application() {
        let src = r#"
            record pair<a, b> {
                first: a,
                second: b,
            }
            variant tree<t> {
                leaf(t),
                branch(list<tree<t>>),
            }
            type boxed<t> = tree<t>
        "#;
        let reg = parse_pact(src).expect("parse generic pact");

        let pair = reg.types.iter().find(|t| t.name() == "pair").unwrap();
        assert_eq!(pair.type_params(), ["a", "b"]);
        match pair {
            TypeDef::Record { fields, .. } => {
                assert_eq!(fields[0].1, Type::Named("a".into()));
                assert_eq!(fields[1].1, Type::Named("b".into()));
            }
            _ => panic!("expected record"),
        }

        let tree = reg.types.iter().find(|t| t.name() == "tree").unwrap();
        assert_eq!(tree.type_params(), ["t"]);
        match tree {
            TypeDef::Variant { cases, .. } => {
                let branch = cases.iter().find(|c| c.name == "branch").unwrap();
                assert_eq!(
                    branch.payload,
                    Some(Type::List(Box::new(Type::App {
                        name: "tree".into(),
                        args: vec![Type::Named("t".into())],
                    })))
                );
            }
            _ => panic!("expected variant"),
        }

        let boxed = reg.types.iter().find(|t| t.name() == "boxed").unwrap();
        assert_eq!(boxed.type_params(), ["t"]);
    }

    #[test]
    fn instantiate_substitutes_params() {
        let td = TypeDef::Record {
            name: "pair".into(),
            type_params: vec!["a".into(), "b".into()],
            fields: vec![
                ("first".into(), Type::Named("a".into())),
                ("second".into(), Type::Named("b".into())),
            ],
        };
        let inst = td.instantiate(&[Type::U32, Type::String]);
        match inst {
            TypeDef::Record {
                type_params,
                fields,
                ..
            } => {
                assert!(type_params.is_empty());
                assert_eq!(fields[0].1, Type::U32);
                assert_eq!(fields[1].1, Type::String);
            }
            _ => panic!("expected record"),
        }
    }
}
