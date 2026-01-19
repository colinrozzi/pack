//! Minimal WIT+ parser for proc macro use.
//!
//! This is a simplified version of the WIT+ parser that runs at compile time
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

/// Registry of all parsed WIT content
#[derive(Debug, Clone, Default)]
pub struct WitRegistry {
    /// Current package declaration (namespace:package)
    pub current_package: Option<(String, String)>,
    /// All interfaces indexed by their full path
    pub interfaces: HashMap<String, Interface>,
    /// All worlds
    pub worlds: Vec<World>,
    /// All top-level type definitions (for the current package)
    pub types: Vec<TypeDef>,
}

impl WitRegistry {
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
                        if *name == path.interface.interface || path.interface.to_string() == *name {
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
                    WorldItem::InterfacePath { namespace, package, interface } => {
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
}

/// A WIT+ type reference
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    // Primitives
    Bool,
    U8, U16, U32, U64,
    S8, S16, S32, S64,
    F32, F64,
    Char,
    String,

    // Compound
    List(Box<Type>),
    Option(Box<Type>),
    Result { ok: Option<Box<Type>>, err: Option<Box<Type>> },
    Tuple(Vec<Type>),

    // Named reference (to another type)
    Named(String),

    // Self-reference within a type definition (for recursion)
    SelfRef,
}

/// A type definition
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// type foo = bar
    Alias { name: String, ty: Type },

    /// record foo { field: type, ... }
    Record { name: String, fields: Vec<(String, Type)> },

    /// variant foo { case(payload), ... }
    Variant { name: String, cases: Vec<VariantCase> },

    /// enum foo { a, b, c }
    Enum { name: String, cases: Vec<String> },

    /// flags foo { a, b, c }
    Flags { name: String, flags: Vec<String> },
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

/// A parsed WIT+ interface
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
    InterfacePath { namespace: Option<String>, package: Option<String>, interface: String },

    /// Inline interface: `name { func... }`
    InlineInterface { name: String, functions: Vec<Function> },
}

/// A parsed WIT+ world
#[derive(Debug, Clone)]
pub struct World {
    pub name: String,
    pub types: Vec<TypeDef>,
    pub imports: Vec<WorldItem>,
    pub exports: Vec<WorldItem>,
}

/// Parse error
#[derive(Debug)]
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
enum Token {
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
                    while let Some(c) = self.chars.next() {
                        if c == '\n' { break; }
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
            if matches!(ch, '{' | '}' | '(' | ')' | '<' | '>' | ':' | ',' | '=' | ';' | '-') {
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

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
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

    fn accept_symbol(&mut self, expected: char) -> bool {
        if matches!(self.peek(), Token::Symbol(c) if *c == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_symbol(&mut self, expected: char) -> Result<(), ParseError> {
        match self.next() {
            Token::Symbol(c) if c == expected => Ok(()),
            other => Err(ParseError::new(format!("expected '{}', got {:?}", expected, other))),
        }
    }

    fn accept_ident(&mut self, expected: &str) -> bool {
        if matches!(self.peek(), Token::Ident(s) if s == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.next() {
            Token::Ident(s) => Ok(s),
            other => Err(ParseError::new(format!("expected identifier, got {:?}", other))),
        }
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }
}

// ============================================================================
// Public parsing functions
// ============================================================================

/// Parse a WIT+ world definition
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
            _ => return Err(ParseError::new(format!("expected 'import' or 'export', got '{}'", keyword))),
        }
    }

    parser.expect_symbol('}')?;

    Ok(World { name, types, imports, exports })
}

/// Parse WIT content and return a complete registry
pub fn parse_wit(src: &str) -> Result<WitRegistry, ParseError> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);

    let mut registry = WitRegistry::default();

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
        if let (Token::Ident(func_name), Token::Symbol(':')) =
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

    Ok(Interface { name, types, functions })
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
            _ => return Err(ParseError::new(format!("expected 'import' or 'export', got '{}'", keyword))),
        }
    }

    parser.expect_symbol('}')?;

    Ok(World { name, types, imports, exports })
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
            parser.expect_symbol('=')?;
            let ty = parse_type(parser)?;
            Ok(Some(TypeDef::Alias { name, ty }))
        }
        "record" => {
            parser.next();
            let name = parser.expect_ident()?;
            parser.expect_symbol('{')?;
            let mut fields = Vec::new();
            while !parser.accept_symbol('}') {
                let field_name = parser.expect_ident()?;
                parser.expect_symbol(':')?;
                let field_type = parse_type(parser)?;
                fields.push((field_name, field_type));
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Record { name, fields }))
        }
        "variant" => {
            parser.next();
            let name = parser.expect_ident()?;
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
                cases.push(VariantCase { name: case_name, payload });
                parser.accept_symbol(',');
            }
            Ok(Some(TypeDef::Variant { name, cases }))
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
        return Ok(WorldItem::InlineInterface { name: first, functions });
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
        if let (Token::Ident(name), Token::Symbol(':'), Token::Ident(func_kw)) =
            (parser.peek().clone(), parser.peek_n(1).clone(), parser.peek_n(2).clone())
        {
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

fn parse_func_signature(parser: &mut Parser, name: String) -> Result<Function, ParseError> {
    parser.expect_symbol('(')?;
    let params = parse_params(parser)?;
    parser.expect_symbol(')')?;

    let results = if parser.accept_symbol('-') {
        parser.expect_symbol('>')?;
        parse_results(parser)?
    } else {
        Vec::new()
    };

    Ok(Function { name, params, results })
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

fn parse_type(parser: &mut Parser) -> Result<Type, ParseError> {
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
        _ => Ok(Type::Named(ident)),
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
            TypeDef::Variant { name, cases } => {
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
            TypeDef::Record { name, fields } => {
                assert_eq!(name, "point");
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("expected record"),
        }
    }
}
