//! Pact Parser
//!
//! Parses Pact interface definitions - Theater's answer to WIT.
//!
//! Pact features:
//! - First-class interfaces with imports/exports
//! - Metadata annotations (@name: Type = value)
//! - Generic type parameters with interface constraints
//! - Nested interfaces for namespacing

use super::{Arena, Case, Field, Function, Param, ParseError, Type, TypeDef};
use std::collections::HashMap;
use std::path::Path;

// ============================================================================
// Pact AST Types
// ============================================================================

/// A parsed Pact interface definition.
#[derive(Debug, Clone)]
pub struct PactInterface {
    /// Interface name
    pub name: String,
    /// Metadata annotations
    pub metadata: Vec<Metadata>,
    /// Use declarations (brings types from other interfaces into scope)
    pub uses: Vec<PactUse>,
    /// Type parameters (generics)
    pub type_params: Vec<TypeParam>,
    /// Type definitions
    pub types: Vec<TypeDef>,
    /// Imported items
    pub imports: Vec<PactImport>,
    /// Exported items
    pub exports: Vec<PactExport>,
    /// Nested interfaces
    pub children: Vec<PactInterface>,
}

impl PactInterface {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            metadata: Vec::new(),
            uses: Vec::new(),
            type_params: Vec::new(),
            types: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Convert this PactInterface to an Arena representation.
    ///
    /// Note: Metadata and type parameters are not included in Arena
    /// (they're Pact-specific). Use `metadata()` and `type_params`
    /// directly if you need them.
    pub fn to_arena(&self) -> Arena {
        let mut arena = Arena::new(&self.name);

        // Add type definitions
        for typedef in &self.types {
            arena.add_type(typedef.clone());
        }

        // Add imports as a child arena
        if !self.imports.is_empty() {
            let mut imports_arena = Arena::new("imports");

            for import in &self.imports {
                match import {
                    PactImport::Interface(name) => {
                        imports_arena.add_child(Arena::new(name));
                    }
                    PactImport::Item { interface, name } => {
                        // Find or create interface child
                        let child = imports_arena
                            .children
                            .iter_mut()
                            .find(|c| c.name == *interface);
                        if let Some(child) = child {
                            // Add placeholder function (actual signature unknown)
                            child.add_function(Function::new(name));
                        } else {
                            let mut new_child = Arena::new(interface);
                            new_child.add_function(Function::new(name));
                            imports_arena.add_child(new_child);
                        }
                    }
                    PactImport::TypeConstraint { param, interface } => {
                        // Type constraints are stored in type_params, not arena
                        // But we can add them as a special marker
                        let constraint_arena = Arena::new(format!("{}:{}", param, interface));
                        imports_arena.add_child(constraint_arena);
                    }
                }
            }

            arena.add_child(imports_arena);
        }

        // Add exports as a child arena
        if !self.exports.is_empty() {
            let mut exports_arena = Arena::new("exports");
            let mut standalone = Arena::new("standalone");

            for export in &self.exports {
                match export {
                    PactExport::Function(func) => {
                        standalone.add_function(func.clone());
                    }
                    PactExport::Type(typedef) => {
                        exports_arena.add_type(typedef.clone());
                    }
                }
            }

            if !standalone.functions.is_empty() {
                exports_arena.add_child(standalone);
            }

            arena.add_child(exports_arena);
        }

        // Add nested interfaces as children
        for child in &self.children {
            arena.add_child(child.to_arena());
        }

        arena
    }

    /// Get metadata value by name.
    pub fn get_metadata(&self, name: &str) -> Option<&Metadata> {
        self.metadata.iter().find(|m| m.name == name)
    }

    /// Get the version metadata if present.
    pub fn version(&self) -> Option<&str> {
        self.get_metadata("version").and_then(|m| {
            if let MetadataValue::String(s) = &m.value {
                Some(s.as_str())
            } else {
                None
            }
        })
    }
}

/// A metadata annotation (@name: Type = value).
#[derive(Debug, Clone)]
pub struct Metadata {
    pub name: String,
    pub ty: Type,
    pub value: MetadataValue,
}

/// A metadata value (literals only for now).
#[derive(Debug, Clone)]
pub enum MetadataValue {
    String(String),
    Bool(bool),
    U64(u64),
    S64(i64),
    F64(f64),
    Record(HashMap<String, MetadataValue>),
}

/// A type parameter declaration.
#[derive(Debug, Clone)]
pub struct TypeParam {
    /// Parameter name
    pub name: String,
    /// Constraint (interface this type must satisfy)
    pub constraint: Option<String>,
}

/// A use declaration for bringing types into scope.
#[derive(Debug, Clone)]
pub struct PactUse {
    /// Source interface name
    pub interface: String,
    /// Items to bring into scope (empty means all)
    pub items: Vec<String>,
}

/// An import declaration.
#[derive(Debug, Clone)]
pub enum PactImport {
    /// Import an interface: `logger`
    Interface(String),
    /// Import a specific item: `logger.log`
    Item { interface: String, name: String },
    /// Type parameter constraint: `T: Serializable`
    TypeConstraint { param: String, interface: String },
}

/// An export declaration.
#[derive(Debug, Clone)]
pub enum PactExport {
    /// Export a function
    Function(Function),
    /// Export a type
    Type(TypeDef),
}

// ============================================================================
// Tokenizer
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    String(String),
    Number(String),
    Symbol(char),
    At,
    Eof,
}

fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();

    while let Some(&ch) = chars.peek() {
        // Skip whitespace
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        // Comments
        if ch == '/' {
            chars.next();
            if matches!(chars.peek(), Some('/')) {
                // Line comment
                while let Some(next) = chars.next() {
                    if next == '\n' {
                        break;
                    }
                }
                continue;
            }
            // Standalone / is a symbol
            tokens.push(Token::Symbol('/'));
            continue;
        }

        // @ for metadata
        if ch == '@' {
            tokens.push(Token::At);
            chars.next();
            continue;
        }

        // String literals
        if ch == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(&next) = chars.peek() {
                if next == '"' {
                    chars.next();
                    break;
                }
                if next == '\\' {
                    chars.next();
                    if let Some(&escaped) = chars.peek() {
                        match escaped {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            _ => s.push(escaped),
                        }
                        chars.next();
                    }
                } else {
                    s.push(next);
                    chars.next();
                }
            }
            tokens.push(Token::String(s));
            continue;
        }

        // Numbers
        if ch.is_ascii_digit() || (ch == '-' && matches!(chars.clone().nth(1), Some(c) if c.is_ascii_digit())) {
            let mut num = String::new();
            if ch == '-' {
                num.push(ch);
                chars.next();
            }
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == '.' {
                    num.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Number(num));
            continue;
        }

        // Identifiers
        if is_ident_start(ch) {
            let mut ident = String::new();
            while let Some(&next) = chars.peek() {
                if is_ident_continue(next) {
                    ident.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Ident(ident));
            continue;
        }

        // Symbols
        if is_symbol(ch) {
            tokens.push(Token::Symbol(ch));
            chars.next();
            continue;
        }

        return Err(ParseError::UnexpectedToken(ch.to_string()));
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn is_symbol(ch: char) -> bool {
    matches!(
        ch,
        '{' | '}' | '(' | ')' | '<' | '>' | ':' | ',' | '=' | ';' | '-' | '.'
    )
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

    fn is_eof(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn next(&mut self) -> Token {
        let tok = self.peek().clone();
        if !matches!(tok, Token::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn expect_symbol(&mut self, expected: char) -> Result<(), ParseError> {
        match self.next() {
            Token::Symbol(ch) if ch == expected => Ok(()),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
            Token::Ident(name) => Err(ParseError::UnexpectedToken(name)),
            Token::String(s) => Err(ParseError::UnexpectedToken(format!("\"{}\"", s))),
            Token::Number(n) => Err(ParseError::UnexpectedToken(n)),
            Token::At => Err(ParseError::UnexpectedToken("@".to_string())),
            Token::Eof => Err(ParseError::UnexpectedEof),
        }
    }

    fn accept_symbol(&mut self, expected: char) -> bool {
        if matches!(self.peek(), Token::Symbol(ch) if *ch == expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.next() {
            Token::Ident(name) => Ok(name),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
            Token::String(s) => Err(ParseError::UnexpectedToken(format!("\"{}\"", s))),
            Token::Number(n) => Err(ParseError::UnexpectedToken(n)),
            Token::At => Err(ParseError::UnexpectedToken("@".to_string())),
            Token::Eof => Err(ParseError::UnexpectedEof),
        }
    }

    fn accept_ident(&mut self, expected: &str) -> bool {
        matches!(self.peek(), Token::Ident(name) if name == expected)
            && {
                self.pos += 1;
                true
            }
    }

    fn accept_at(&mut self) -> bool {
        if matches!(self.peek(), Token::At) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    fn expect_string(&mut self) -> Result<String, ParseError> {
        match self.next() {
            Token::String(s) => Ok(s),
            Token::Ident(name) => Err(ParseError::UnexpectedToken(name)),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
            Token::Number(n) => Err(ParseError::UnexpectedToken(n)),
            Token::At => Err(ParseError::UnexpectedToken("@".to_string())),
            Token::Eof => Err(ParseError::UnexpectedEof),
        }
    }

    fn expect_eof(&mut self) -> Result<(), ParseError> {
        match self.next() {
            Token::Eof => Ok(()),
            Token::Ident(name) => Err(ParseError::UnexpectedToken(name)),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
            Token::String(s) => Err(ParseError::UnexpectedToken(format!("\"{}\"", s))),
            Token::Number(n) => Err(ParseError::UnexpectedToken(n)),
            Token::At => Err(ParseError::UnexpectedToken("@".to_string())),
        }
    }
}

// ============================================================================
// Parse Functions
// ============================================================================

/// Parse a Pact interface definition from source string.
pub fn parse_pact(src: &str) -> Result<PactInterface, ParseError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser::new(tokens);

    // Expect `interface name { ... }`
    if parser.accept_ident("interface") {
        let interface = parse_interface(&mut parser)?;
        parser.expect_eof()?;
        return Ok(interface);
    }

    // Or parse as anonymous root interface
    let mut interface = PactInterface::new("root");
    parse_interface_body(&mut parser, &mut interface)?;
    parser.expect_eof()?;
    Ok(interface)
}

/// Parse a single .pact file from disk.
///
/// The interface name will be derived from the filename (without .pact extension).
pub fn parse_pact_file(path: impl AsRef<Path>) -> Result<PactInterface, PactFileError> {
    let path = path.as_ref();
    let src = std::fs::read_to_string(path)
        .map_err(|e| PactFileError::Io(path.to_path_buf(), e))?;

    let mut interface = parse_pact(&src)
        .map_err(|e| PactFileError::Parse(path.to_path_buf(), e))?;

    // If the interface was parsed as "root", use the filename as the name
    if interface.name == "root" {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            interface.name = stem.to_string();
        }
    }

    Ok(interface)
}

/// Parse all .pact files in a directory.
///
/// Returns a root interface containing all parsed interfaces as children.
/// The directory structure is preserved in nested interfaces.
pub fn parse_pact_dir(path: impl AsRef<Path>) -> Result<PactInterface, PactFileError> {
    let path = path.as_ref();

    if !path.is_dir() {
        return Err(PactFileError::NotADirectory(path.to_path_buf()));
    }

    let dir_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("root");

    let mut root = PactInterface::new(dir_name);

    parse_pact_dir_recursive(path, &mut root)?;

    Ok(root)
}

fn parse_pact_dir_recursive(
    dir: &Path,
    parent: &mut PactInterface,
) -> Result<(), PactFileError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| PactFileError::Io(dir.to_path_buf(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| PactFileError::Io(dir.to_path_buf(), e))?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectory
            let subdir_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");

            let mut child = PactInterface::new(subdir_name);
            parse_pact_dir_recursive(&path, &mut child)?;

            if !child.children.is_empty() || !child.types.is_empty()
                || !child.exports.is_empty() || !child.imports.is_empty() {
                parent.children.push(child);
            }
        } else if path.extension().and_then(|s| s.to_str()) == Some("pact") {
            // Parse .pact file
            let interface = parse_pact_file(&path)?;
            parent.children.push(interface);
        }
    }

    Ok(())
}

/// Error type for file-based Pact parsing.
#[derive(Debug)]
pub enum PactFileError {
    /// IO error reading file or directory
    Io(std::path::PathBuf, std::io::Error),
    /// Parse error in a specific file
    Parse(std::path::PathBuf, ParseError),
    /// Path is not a directory
    NotADirectory(std::path::PathBuf),
}

impl std::fmt::Display for PactFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PactFileError::Io(path, err) => {
                write!(f, "IO error for {}: {}", path.display(), err)
            }
            PactFileError::Parse(path, err) => {
                write!(f, "Parse error in {}: {}", path.display(), err)
            }
            PactFileError::NotADirectory(path) => {
                write!(f, "Not a directory: {}", path.display())
            }
        }
    }
}

impl std::error::Error for PactFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PactFileError::Io(_, err) => Some(err),
            PactFileError::Parse(_, err) => Some(err),
            PactFileError::NotADirectory(_) => None,
        }
    }
}

// ============================================================================
// Type Registry for Cross-File Resolution
// ============================================================================

/// A registry for resolving types across multiple interfaces.
///
/// The registry maps interface names to their exported types, allowing
/// `use` statements to be resolved across files.
#[derive(Debug, Clone, Default)]
pub struct TypeRegistry {
    /// Map from interface name to its type definitions
    interfaces: HashMap<String, InterfaceTypes>,
}

/// Types exported by an interface.
#[derive(Debug, Clone, Default)]
pub struct InterfaceTypes {
    /// The interface itself
    pub interface: Option<PactInterface>,
    /// Types defined in this interface (name -> TypeDef)
    pub types: HashMap<String, TypeDef>,
}

impl TypeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from a parsed directory of pact files.
    pub fn from_interface(root: &PactInterface) -> Self {
        let mut registry = Self::new();
        registry.add_interface(root);
        registry
    }

    /// Add an interface and all its children to the registry.
    pub fn add_interface(&mut self, interface: &PactInterface) {
        let mut iface_types = InterfaceTypes::default();
        iface_types.interface = Some(interface.clone());

        // Add all types from this interface
        for typedef in &interface.types {
            iface_types.types.insert(typedef.name().to_string(), typedef.clone());
        }

        self.interfaces.insert(interface.name.clone(), iface_types);

        // Recursively add children
        for child in &interface.children {
            self.add_interface(child);
        }
    }

    /// Look up a type by interface and name.
    pub fn get_type(&self, interface: &str, name: &str) -> Option<&TypeDef> {
        self.interfaces
            .get(interface)
            .and_then(|iface| iface.types.get(name))
    }

    /// Look up an interface by name.
    pub fn get_interface(&self, name: &str) -> Option<&PactInterface> {
        self.interfaces
            .get(name)
            .and_then(|iface| iface.interface.as_ref())
    }

    /// List all interfaces in the registry.
    pub fn interfaces(&self) -> impl Iterator<Item = &str> {
        self.interfaces.keys().map(|s| s.as_str())
    }

    /// List all types in an interface.
    pub fn types_in(&self, interface: &str) -> Option<impl Iterator<Item = &str>> {
        self.interfaces
            .get(interface)
            .map(|iface| iface.types.keys().map(|s| s.as_str()))
    }

    /// Resolve a `use` declaration and return the types it brings into scope.
    ///
    /// If `use_decl.items` is empty, returns all types from the interface.
    /// Otherwise, returns only the specified types.
    pub fn resolve_use(&self, use_decl: &PactUse) -> Result<Vec<(String, TypeDef)>, String> {
        let iface = self.interfaces
            .get(&use_decl.interface)
            .ok_or_else(|| format!("Unknown interface: {}", use_decl.interface))?;

        if use_decl.items.is_empty() {
            // Import all types
            Ok(iface.types.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        } else {
            // Import specific types
            let mut result = Vec::new();
            for item in &use_decl.items {
                let typedef = iface.types
                    .get(item)
                    .ok_or_else(|| format!("Type {} not found in interface {}", item, use_decl.interface))?;
                result.push((item.clone(), typedef.clone()));
            }
            Ok(result)
        }
    }

    /// Create a resolved scope for an interface, including its own types
    /// and all types brought in via `use` statements.
    pub fn resolve_scope(&self, interface: &PactInterface) -> Result<HashMap<String, TypeDef>, String> {
        let mut scope = HashMap::new();

        // Add the interface's own types
        for typedef in &interface.types {
            scope.insert(typedef.name().to_string(), typedef.clone());
        }

        // Resolve each `use` and add those types
        for use_decl in &interface.uses {
            let imported = self.resolve_use(use_decl)?;
            for (name, typedef) in imported {
                scope.insert(name, typedef);
            }
        }

        Ok(scope)
    }
}

/// Parse a directory and build both the interface tree and type registry.
pub fn parse_pact_dir_with_registry(
    path: impl AsRef<Path>,
) -> Result<(PactInterface, TypeRegistry), PactFileError> {
    let interface = parse_pact_dir(path)?;
    let registry = TypeRegistry::from_interface(&interface);
    Ok((interface, registry))
}

fn parse_interface(parser: &mut Parser) -> Result<PactInterface, ParseError> {
    let name = parser.expect_ident()?;
    let mut interface = PactInterface::new(name);

    parser.expect_symbol('{')?;
    parse_interface_body(parser, &mut interface)?;
    parser.expect_symbol('}')?;

    Ok(interface)
}

fn parse_interface_body(parser: &mut Parser, interface: &mut PactInterface) -> Result<(), ParseError> {
    while !parser.is_eof() {
        // Skip semicolons
        if parser.accept_symbol(';') {
            continue;
        }

        // Check for closing brace
        if matches!(parser.peek(), Token::Symbol('}')) {
            break;
        }

        // Metadata annotation
        if parser.accept_at() {
            let meta = parse_metadata(parser)?;
            interface.metadata.push(meta);
            continue;
        }

        // Keywords
        let keyword = parser.expect_ident()?;
        match keyword.as_str() {
            "type" => {
                let type_item = parse_type_item(parser)?;
                match type_item {
                    TypeItem::TypeParam(tp) => interface.type_params.push(tp),
                    TypeItem::TypeDef(td) => interface.types.push(td),
                }
            }
            "record" => interface.types.push(parse_record(parser)?),
            "variant" => interface.types.push(parse_variant(parser)?),
            "enum" => interface.types.push(parse_enum(parser)?),
            "flags" => interface.types.push(parse_flags(parser)?),
            "imports" => {
                parser.expect_symbol('{')?;
                parse_imports_block(parser, interface)?;
                parser.expect_symbol('}')?;
            }
            "exports" => {
                parser.expect_symbol('{')?;
                parse_exports_block(parser, interface)?;
                parser.expect_symbol('}')?;
            }
            "interface" => {
                let child = parse_interface(parser)?;
                interface.children.push(child);
            }
            "use" => {
                let use_decl = parse_use(parser)?;
                interface.uses.push(use_decl);
            }
            _ => return Err(ParseError::UnexpectedToken(keyword)),
        }
    }

    Ok(())
}

fn parse_metadata(parser: &mut Parser) -> Result<Metadata, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol(':')?;
    let ty = parse_type(parser)?;
    parser.expect_symbol('=')?;
    let value = parse_metadata_value(parser)?;
    Ok(Metadata { name, ty, value })
}

fn parse_metadata_value(parser: &mut Parser) -> Result<MetadataValue, ParseError> {
    match parser.peek().clone() {
        Token::String(s) => {
            parser.next();
            Ok(MetadataValue::String(s))
        }
        Token::Number(n) => {
            parser.next();
            if n.contains('.') {
                n.parse::<f64>()
                    .map(MetadataValue::F64)
                    .map_err(|_| ParseError::UnexpectedToken(n))
            } else if n.starts_with('-') {
                n.parse::<i64>()
                    .map(MetadataValue::S64)
                    .map_err(|_| ParseError::UnexpectedToken(n))
            } else {
                n.parse::<u64>()
                    .map(MetadataValue::U64)
                    .map_err(|_| ParseError::UnexpectedToken(n))
            }
        }
        Token::Ident(name) if name == "true" => {
            parser.next();
            Ok(MetadataValue::Bool(true))
        }
        Token::Ident(name) if name == "false" => {
            parser.next();
            Ok(MetadataValue::Bool(false))
        }
        Token::Symbol('{') => {
            parser.next();
            let mut fields = HashMap::new();
            while !parser.accept_symbol('}') {
                let field_name = parser.expect_ident()?;
                parser.expect_symbol(':')?;
                let field_value = parse_metadata_value(parser)?;
                fields.insert(field_name, field_value);
                parser.accept_symbol(',');
            }
            Ok(MetadataValue::Record(fields))
        }
        _ => Err(ParseError::UnexpectedToken(format!("{:?}", parser.peek()))),
    }
}

enum TypeItem {
    TypeParam(TypeParam),
    TypeDef(TypeDef),
}

fn parse_type_item(parser: &mut Parser) -> Result<TypeItem, ParseError> {
    let name = parser.expect_ident()?;

    // Check for type parameter constraint: `type T: Constraint`
    if parser.accept_symbol(':') {
        let constraint = parser.expect_ident()?;
        return Ok(TypeItem::TypeParam(TypeParam {
            name,
            constraint: Some(constraint),
        }));
    }

    // Check for type alias: `type Foo = bar`
    if parser.accept_symbol('=') {
        let ty = parse_type(parser)?;
        return Ok(TypeItem::TypeDef(TypeDef::alias(name, ty)));
    }

    // Just a type parameter with no constraint
    Ok(TypeItem::TypeParam(TypeParam {
        name,
        constraint: None,
    }))
}

fn parse_record(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut fields = Vec::new();

    while !parser.accept_symbol('}') {
        let field_name = parser.expect_ident()?;
        parser.expect_symbol(':')?;
        let field_type = parse_type(parser)?;
        fields.push(Field::new(field_name, field_type));
        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }

    Ok(TypeDef::record(name, fields))
}

fn parse_variant(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut cases = Vec::new();

    while !parser.accept_symbol('}') {
        let case_name = parser.expect_ident()?;
        let case = if parser.accept_symbol('(') {
            let ty = parse_type(parser)?;
            parser.expect_symbol(')')?;
            Case::new(case_name, ty)
        } else {
            Case::unit(case_name)
        };
        cases.push(case);
        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }

    Ok(TypeDef::variant(name, cases))
}

fn parse_enum(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut cases = Vec::new();

    while !parser.accept_symbol('}') {
        let case = parser.expect_ident()?;
        cases.push(case);
        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }

    Ok(TypeDef::enumeration(name, cases))
}

fn parse_flags(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut flags = Vec::new();

    while !parser.accept_symbol('}') {
        let flag = parser.expect_ident()?;
        flags.push(flag);
        parser.accept_symbol(',');
        parser.accept_symbol(';');
    }

    Ok(TypeDef::flags(name, flags))
}

fn parse_use(parser: &mut Parser) -> Result<PactUse, ParseError> {
    // Parse: use <interface>.{item1, item2, ...}
    // Or:    use <interface>
    let interface_name = parser.expect_ident()?;

    let mut items = Vec::new();

    // Check for dot-delimited items: .{item1, item2}
    if parser.accept_symbol('.') {
        parser.expect_symbol('{')?;
        while !parser.accept_symbol('}') {
            let item = parser.expect_ident()?;
            items.push(item);
            if !parser.accept_symbol(',') {
                // Allow trailing comma or no comma before }
                if !matches!(parser.peek(), Token::Symbol('}')) {
                    parser.expect_symbol(',')?;
                }
            }
        }
    }

    Ok(PactUse {
        interface: interface_name,
        items,
    })
}

fn parse_imports_block(parser: &mut Parser, interface: &mut PactInterface) -> Result<(), ParseError> {
    while !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        let first = parser.expect_ident()?;

        // Type constraint: `T: Constraint`
        if parser.accept_symbol(':') {
            let constraint = parser.expect_ident()?;
            interface.imports.push(PactImport::TypeConstraint {
                param: first,
                interface: constraint,
            });
            continue;
        }

        // Item import: `logger.log`
        if parser.accept_symbol('.') {
            let item = parser.expect_ident()?;
            interface.imports.push(PactImport::Item {
                interface: first,
                name: item,
            });
            continue;
        }

        // Interface import: `logger`
        interface.imports.push(PactImport::Interface(first));
    }

    Ok(())
}

fn parse_exports_block(parser: &mut Parser, interface: &mut PactInterface) -> Result<(), ParseError> {
    while !matches!(parser.peek(), Token::Symbol('}')) {
        if parser.accept_symbol(';') {
            continue;
        }

        let name = parser.expect_ident()?;

        // Function: `name: func(...) -> ...`
        if parser.accept_symbol(':') {
            if parser.accept_ident("func") {
                let func = parse_func_signature(parser, name)?;
                interface.exports.push(PactExport::Function(func));
                continue;
            }
            // Could be a type alias in exports
            let ty = parse_type(parser)?;
            interface.exports.push(PactExport::Type(TypeDef::alias(name, ty)));
            continue;
        }

        return Err(ParseError::UnexpectedToken(name));
    }

    Ok(())
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

    Ok(Function::with_signature(name, params, results))
}

fn parse_params(parser: &mut Parser) -> Result<Vec<Param>, ParseError> {
    let mut params = Vec::new();
    if matches!(parser.peek(), Token::Symbol(')')) {
        return Ok(params);
    }

    loop {
        let name = parser.expect_ident()?;
        parser.expect_symbol(':')?;
        let ty = parse_type(parser)?;
        params.push(Param::new(name, ty));
        if matches!(parser.peek(), Token::Symbol(')')) {
            break;
        }
        parser.expect_symbol(',')?;
    }

    Ok(params)
}

fn parse_results(parser: &mut Parser) -> Result<Vec<Type>, ParseError> {
    // Single type
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
        "self" => Ok(Type::self_ref()),
        "value" => Ok(Type::Value),
        "_" => Ok(Type::Tuple(vec![])), // Unit type, used in result<_, E> for void ok type
        "list" => parse_generic_type(parser, |t| Type::list(t)),
        "option" => parse_generic_type(parser, |t| Type::option(t)),
        "tuple" => parse_tuple(parser),
        "result" => parse_result(parser),
        _ => Ok(Type::named(ident)),
    }
}

fn parse_generic_type<F>(parser: &mut Parser, wrap: F) -> Result<Type, ParseError>
where
    F: Fn(Type) -> Type,
{
    parser.expect_symbol('<')?;
    let inner = parse_type(parser)?;
    parser.expect_symbol('>')?;
    Ok(wrap(inner))
}

fn parse_tuple(parser: &mut Parser) -> Result<Type, ParseError> {
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

fn parse_result(parser: &mut Parser) -> Result<Type, ParseError> {
    parser.expect_symbol('<')?;
    // For compatibility with pack-guest-macros, `_` maps to Bool for ok type
    let ok = if parser.accept_ident("_") {
        Type::Bool
    } else {
        parse_type(parser)?
    };
    parser.expect_symbol(',')?;
    // For compatibility with pack-guest-macros, `_` maps to String for err type
    let err = if parser.accept_ident("_") {
        Type::String
    } else {
        parse_type(parser)?
    };
    parser.expect_symbol('>')?;
    Ok(Type::result(ok, err))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_interface() {
        let src = r#"
            interface calculator {
                @version: string = "1.0.0";

                exports {
                    add: func(a: s32, b: s32) -> s32;
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.name, "calculator");
        assert_eq!(interface.metadata.len(), 1);
        assert_eq!(interface.metadata[0].name, "version");
        assert_eq!(interface.exports.len(), 1);
    }

    #[test]
    fn parse_interface_with_imports() {
        let src = r#"
            interface calculator {
                imports {
                    logger;
                    types.BigNum;
                }

                exports {
                    add: func(a: s32, b: s32) -> s32;
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.imports.len(), 2);
    }

    #[test]
    fn parse_generic_interface() {
        let src = r#"
            interface storage {
                type T: Serializable;

                exports {
                    get: func() -> T;
                    set: func(value: T);
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.type_params.len(), 1);
        assert_eq!(interface.type_params[0].name, "T");
        assert_eq!(interface.type_params[0].constraint, Some("Serializable".to_string()));
    }

    #[test]
    fn parse_nested_interfaces() {
        let src = r#"
            interface my-org {
                interface calculator {
                    exports {
                        add: func(a: s32, b: s32) -> s32;
                    }
                }

                interface logger {
                    exports {
                        log: func(msg: string);
                    }
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.name, "my-org");
        assert_eq!(interface.children.len(), 2);
        assert_eq!(interface.children[0].name, "calculator");
        assert_eq!(interface.children[1].name, "logger");
    }

    #[test]
    fn parse_types() {
        let src = r#"
            interface types {
                record point {
                    x: f32;
                    y: f32;
                }

                variant shape {
                    circle(f32),
                    rectangle(tuple<f32, f32>),
                    point,
                }

                enum color {
                    red,
                    green,
                    blue,
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.types.len(), 3);
    }

    #[test]
    fn parse_metadata_values() {
        let src = r#"
            interface test {
                @version: string = "1.2.3";
                @count: u32 = 42;
                @enabled: bool = true;
                @config: Config = { timeout: 30, debug: false };
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.metadata.len(), 4);
    }

    #[test]
    fn convert_to_arena() {
        let src = r#"
            interface calculator {
                @version: string = "1.0.0";

                record point {
                    x: f32;
                    y: f32;
                }

                imports {
                    logger;
                }

                exports {
                    add: func(a: s32, b: s32) -> s32;
                    sub: func(a: s32, b: s32) -> s32;
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let arena = interface.to_arena();

        assert_eq!(arena.name, "calculator");
        assert_eq!(arena.types.len(), 1); // point record
        assert_eq!(arena.children.len(), 2); // imports + exports

        // Check version accessor
        assert_eq!(interface.version(), Some("1.0.0"));
    }

    #[test]
    fn parse_pact_file_test() {
        use std::io::Write;

        // Create a temp file
        let dir = std::env::temp_dir().join("pact_test");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("calculator.pact");

        let src = r#"
            interface calculator {
                @version: string = "1.0.0";

                exports {
                    add: func(a: s32, b: s32) -> s32;
                }
            }
        "#;

        let mut file = std::fs::File::create(&file_path).expect("create file");
        file.write_all(src.as_bytes()).expect("write file");

        let interface = super::parse_pact_file(&file_path).expect("parse file");
        assert_eq!(interface.name, "calculator");
        assert_eq!(interface.version(), Some("1.0.0"));

        // Cleanup
        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn parse_pact_dir_test() {
        use std::io::Write;

        // Create a temp directory structure
        let dir = std::env::temp_dir().join("pact_dir_test");
        let _ = std::fs::remove_dir_all(&dir); // Clean up any previous run
        std::fs::create_dir_all(&dir).expect("create dir");

        // Create calculator.pact
        let calc_src = r#"
            interface calculator {
                exports {
                    add: func(a: s32, b: s32) -> s32;
                }
            }
        "#;
        let mut file = std::fs::File::create(dir.join("calculator.pact")).expect("create");
        file.write_all(calc_src.as_bytes()).expect("write");

        // Create logger.pact
        let logger_src = r#"
            interface logger {
                exports {
                    log: func(msg: string);
                }
            }
        "#;
        let mut file = std::fs::File::create(dir.join("logger.pact")).expect("create");
        file.write_all(logger_src.as_bytes()).expect("write");

        // Parse the directory
        let root = super::parse_pact_dir(&dir).expect("parse dir");

        assert_eq!(root.name, "pact_dir_test");
        assert_eq!(root.children.len(), 2);

        // Children could be in any order
        let names: Vec<_> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"calculator"));
        assert!(names.contains(&"logger"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_interface_to_arena() {
        let src = r#"
            interface my-org {
                interface calculator {
                    exports {
                        add: func(a: s32, b: s32) -> s32;
                    }
                }

                interface logger {
                    exports {
                        log: func(msg: string);
                    }
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        let arena = interface.to_arena();

        assert_eq!(arena.name, "my-org");
        assert_eq!(arena.children.len(), 2);
        assert_eq!(arena.children[0].name, "calculator");
        assert_eq!(arena.children[1].name, "logger");
    }

    #[test]
    fn parse_use_statements() {
        let src = r#"
            interface runtime {
                use types.{chain, actor-id}
                use logger

                exports {
                    get-chain: func() -> chain;
                }
            }
        "#;

        let interface = parse_pact(src).expect("parse");
        assert_eq!(interface.name, "runtime");
        assert_eq!(interface.uses.len(), 2);

        // First use: types.{chain, actor-id}
        assert_eq!(interface.uses[0].interface, "types");
        assert_eq!(interface.uses[0].items.len(), 2);
        assert_eq!(interface.uses[0].items[0], "chain");
        assert_eq!(interface.uses[0].items[1], "actor-id");

        // Second use: logger (no specific items)
        assert_eq!(interface.uses[1].interface, "logger");
        assert!(interface.uses[1].items.is_empty());
    }

    #[test]
    fn type_registry_basic() {
        let types_src = r#"
            interface types {
                record chain {
                    entries: list<u8>,
                }

                type actor-id = string
            }
        "#;

        let runtime_src = r#"
            interface runtime {
                use types.{chain, actor-id}

                exports {
                    get-chain: func() -> chain;
                }
            }
        "#;

        let types_iface = parse_pact(types_src).expect("parse types");
        let runtime_iface = parse_pact(runtime_src).expect("parse runtime");

        // Build a root interface containing both
        let mut root = PactInterface::new("root");
        root.children.push(types_iface);
        root.children.push(runtime_iface);

        let registry = TypeRegistry::from_interface(&root);

        // Check interfaces are registered
        assert!(registry.get_interface("types").is_some());
        assert!(registry.get_interface("runtime").is_some());

        // Check types can be looked up
        assert!(registry.get_type("types", "chain").is_some());
        assert!(registry.get_type("types", "actor-id").is_some());

        // Resolve a use declaration
        let runtime = registry.get_interface("runtime").unwrap();
        let scope = registry.resolve_scope(runtime).expect("resolve scope");

        // Scope should contain imported types
        assert!(scope.contains_key("chain"));
        assert!(scope.contains_key("actor-id"));
    }

    #[test]
    fn type_registry_resolve_all() {
        let types_src = r#"
            interface types {
                record point { x: f32, y: f32 }
                record color { r: u8, g: u8, b: u8 }
            }
        "#;

        let consumer_src = r#"
            interface consumer {
                use types
            }
        "#;

        let types_iface = parse_pact(types_src).expect("parse");
        let consumer_iface = parse_pact(consumer_src).expect("parse");

        let mut root = PactInterface::new("root");
        root.children.push(types_iface);
        root.children.push(consumer_iface);

        let registry = TypeRegistry::from_interface(&root);

        // Consumer uses all types from 'types'
        let consumer = registry.get_interface("consumer").unwrap();
        let scope = registry.resolve_scope(consumer).expect("resolve scope");

        // Should have both point and color
        assert!(scope.contains_key("point"));
        assert!(scope.contains_key("color"));
    }

    #[test]
    fn cross_file_resolution() {
        // Test that runtime can use types from types interface
        let types_src = r#"
            interface types {
                record chain {
                    events: list<u8>,
                }
                type actor-id = string
                record channel-accept {
                    accepted: bool,
                    message: option<list<u8>>,
                }
            }
        "#;

        let runtime_src = r#"
            interface runtime {
                use types.{chain}
                exports {
                    get-chain: func() -> chain;
                }
            }
        "#;

        let client_src = r#"
            interface message-server-client {
                use types.{channel-accept}
                exports {
                    handle: func() -> channel-accept;
                }
            }
        "#;

        let types_iface = parse_pact(types_src).expect("parse types");
        let runtime_iface = parse_pact(runtime_src).expect("parse runtime");
        let client_iface = parse_pact(client_src).expect("parse client");

        let mut root = PactInterface::new("theater");
        root.children.push(types_iface);
        root.children.push(runtime_iface);
        root.children.push(client_iface);

        let registry = TypeRegistry::from_interface(&root);

        // Verify runtime can resolve 'chain'
        let runtime = registry.get_interface("runtime").unwrap();
        let runtime_scope = registry.resolve_scope(runtime).expect("resolve runtime scope");
        assert!(runtime_scope.contains_key("chain"));
        let chain_type = runtime_scope.get("chain").unwrap();
        assert!(matches!(chain_type, TypeDef::Record { .. }));

        // Verify message-server-client can resolve 'channel-accept'
        let client = registry.get_interface("message-server-client").unwrap();
        let client_scope = registry.resolve_scope(client).expect("resolve client scope");
        assert!(client_scope.contains_key("channel-accept"));
        let channel_accept = client_scope.get("channel-accept").unwrap();
        assert!(matches!(channel_accept, TypeDef::Record { .. }));
    }
}
