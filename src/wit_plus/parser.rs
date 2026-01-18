//! Minimal WIT+ parser scaffold.
//!
//! Parses top-level type definitions and validates named references.

use super::{
    EnumDef, FlagsDef, Interface, InterfaceExport, InterfaceImport, InterfacePath, ParseError,
    RecordDef, Type, TypeDef, VariantCase, VariantDef, World, WorldItem,
};

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Symbol(char),
    Eof,
}

pub fn parse_interface(src: &str) -> Result<Interface, ParseError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser::new(tokens);

    if parser.accept_ident("interface") {
        let name = parser.expect_ident()?;
        let mut interface = Interface::new(name);
        parser.expect_symbol('{')?;
        parse_interface_body(&mut parser, &mut interface)?;
        parser.expect_symbol('}')?;
        parser.expect_eof()?;
        interface.validate()?;
        return Ok(interface);
    }

    let mut interface = Interface::new("root");
    parse_interface_body(&mut parser, &mut interface)?;
    parser.expect_eof()?;
    interface.validate()?;
    Ok(interface)
}

/// Parse a WIT+ world definition.
///
/// # Example
///
/// ```
/// use composite::wit_plus::parse_world;
///
/// let src = r#"
///     world my-component {
///         import wasi:cli/stdin
///         import log: func(msg: string)
///         export run: func() -> string
///     }
/// "#;
///
/// let world = parse_world(src).expect("parse");
/// assert_eq!(world.name, "my-component");
/// assert_eq!(world.imports.len(), 2);
/// assert_eq!(world.exports.len(), 1);
/// ```
pub fn parse_world(src: &str) -> Result<World, ParseError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser::new(tokens);

    parser.expect_ident_value("world")?;
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;

    let mut world = World::new(name);
    parse_world_body(&mut parser, &mut world)?;

    parser.expect_symbol('}')?;
    parser.expect_eof()?;
    world.validate()?;
    Ok(world)
}

fn parse_world_body(parser: &mut Parser, world: &mut World) -> Result<(), ParseError> {
    while !parser.is_eof() {
        if parser.accept_symbol(';') {
            continue;
        }

        if matches!(parser.peek(), Token::Symbol('}')) {
            break;
        }

        let keyword = parser.expect_ident()?;
        match keyword.as_str() {
            "import" => {
                let item = parse_world_item(parser)?;
                world.add_import(item);
            }
            "export" => {
                let item = parse_world_item(parser)?;
                world.add_export(item);
            }
            _ => return Err(ParseError::UnexpectedToken(keyword)),
        }
    }

    Ok(())
}

fn parse_world_item(parser: &mut Parser) -> Result<WorldItem, ParseError> {
    // Look ahead to determine what kind of item this is:
    // 1. `name: func(...)` - standalone function
    // 2. `name { ... }` - inline interface
    // 3. `name` or `ns:pkg/name` - interface reference

    let first = parser.expect_ident()?;

    // Check for colon - could be function or namespace
    if parser.accept_symbol(':') {
        // Check if next token is `func` keyword (standalone function)
        if parser.accept_ident("func") {
            let func = parse_func(parser, Some(first))?;
            return Ok(WorldItem::Function(func));
        }

        // Otherwise it's a namespace:package/interface path
        // We already consumed the colon, so parse package/interface
        let package = parser.expect_ident()?;
        parser.expect_symbol('/')?;
        let interface = parser.expect_ident()?;

        return Ok(WorldItem::InterfacePath(InterfacePath::qualified(
            first, package, interface,
        )));
    }

    // Check for opening brace (inline interface)
    if parser.accept_symbol('{') {
        let functions = parse_function_block(parser)?;
        parser.expect_symbol('}')?;
        return Ok(WorldItem::InlineInterface {
            name: first,
            functions,
        });
    }

    // Check for slash (package/interface without namespace)
    if parser.accept_symbol('/') {
        let interface = parser.expect_ident()?;
        return Ok(WorldItem::InterfacePath(InterfacePath {
            namespace: None,
            package: Some(first),
            interface,
        }));
    }

    // Simple interface reference
    Ok(WorldItem::InterfacePath(InterfacePath::simple(first)))
}

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

    fn expect_ident_value(&mut self, expected: &str) -> Result<(), ParseError> {
        match self.peek() {
            Token::Ident(name) if name == expected => {
                self.pos += 1;
                Ok(())
            }
            Token::Ident(name) => Err(ParseError::UnexpectedToken(name.clone())),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
            Token::Eof => Err(ParseError::UnexpectedEof),
        }
    }

    fn peek_n(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
    }

    fn expect_eof(&mut self) -> Result<(), ParseError> {
        match self.next() {
            Token::Eof => Ok(()),
            Token::Ident(name) => Err(ParseError::UnexpectedToken(name)),
            Token::Symbol(ch) => Err(ParseError::UnexpectedToken(ch.to_string())),
        }
    }
}

fn parse_interface_body(parser: &mut Parser, interface: &mut Interface) -> Result<(), ParseError> {
    while !parser.is_eof() {
        if parser.accept_symbol(';') {
            continue;
        }

        if matches!(parser.peek(), Token::Symbol('}')) {
            break;
        }

        if let Some(func) = try_parse_named_func(parser)? {
            interface.add_function(func);
            continue;
        }

        let keyword = parser.expect_ident()?;
        match keyword.as_str() {
            "type" => interface.add_type(parse_alias(parser)?),
            "record" => interface.add_type(parse_record(parser)?),
            "variant" => interface.add_type(parse_variant(parser)?),
            "enum" => interface.add_type(parse_enum(parser)?),
            "flags" => interface.add_type(parse_flags(parser)?),
            "func" => interface.add_function(parse_func(parser, None)?),
            "import" => interface.add_import(parse_import_block(parser)?),
            "export" => interface.add_export(parse_export_block(parser)?),
            _ => return Err(ParseError::UnexpectedToken(keyword)),
        }
    }

    Ok(())
}

fn parse_alias(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('=')?;
    let ty = parse_type(parser)?;
    Ok(TypeDef::Alias(name, ty))
}

fn parse_record(parser: &mut Parser) -> Result<TypeDef, ParseError> {
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

    Ok(TypeDef::Record(RecordDef { name, fields }))
}

fn parse_variant(parser: &mut Parser) -> Result<TypeDef, ParseError> {
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
        cases.push(VariantCase {
            name: case_name,
            payload,
        });
        parser.accept_symbol(',');
    }

    Ok(TypeDef::Variant(VariantDef { name, cases }))
}

fn parse_enum(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut cases = Vec::new();

    while !parser.accept_symbol('}') {
        let case = parser.expect_ident()?;
        cases.push(case);
        parser.accept_symbol(',');
    }

    Ok(TypeDef::Enum(EnumDef { name, cases }))
}

fn parse_flags(parser: &mut Parser) -> Result<TypeDef, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let mut flags = Vec::new();

    while !parser.accept_symbol('}') {
        let flag = parser.expect_ident()?;
        flags.push(flag);
        parser.accept_symbol(',');
    }

    Ok(TypeDef::Flags(FlagsDef { name, flags }))
}

fn try_parse_named_func(parser: &mut Parser) -> Result<Option<super::Function>, ParseError> {
    let (Token::Ident(name), Token::Symbol(':'), Token::Ident(func_kw)) =
        (parser.peek().clone(), parser.peek_n(1).clone(), parser.peek_n(2).clone())
    else {
        return Ok(None);
    };

    if func_kw != "func" {
        return Ok(None);
    }

    parser.next();
    parser.next();
    parser.next();

    let func = parse_func(parser, Some(name))?;
    Ok(Some(func))
}

fn parse_func(
    parser: &mut Parser,
    name_override: Option<String>,
) -> Result<super::Function, ParseError> {
    let name = match name_override {
        Some(name) => name,
        None => parser.expect_ident()?,
    };

    parser.expect_symbol('(')?;
    let params = parse_params(parser)?;
    parser.expect_symbol(')')?;

    let results = if parser.accept_symbol('-') {
        parser.expect_symbol('>')?;
        parse_results(parser)?
    } else {
        Vec::new()
    };

    Ok(super::Function {
        name,
        params,
        results,
    })
}

fn parse_import_block(parser: &mut Parser) -> Result<InterfaceImport, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let functions = parse_function_block(parser)?;
    parser.expect_symbol('}')?;
    Ok(InterfaceImport { name, functions })
}

fn parse_export_block(parser: &mut Parser) -> Result<InterfaceExport, ParseError> {
    let name = parser.expect_ident()?;
    parser.expect_symbol('{')?;
    let functions = parse_function_block(parser)?;
    parser.expect_symbol('}')?;
    Ok(InterfaceExport { name, functions })
}

fn parse_function_block(parser: &mut Parser) -> Result<Vec<super::Function>, ParseError> {
    let mut functions = Vec::new();

    while !parser.is_eof() {
        if parser.accept_symbol(';') {
            continue;
        }
        if matches!(parser.peek(), Token::Symbol('}')) {
            break;
        }
        if let Some(func) = try_parse_named_func(parser)? {
            functions.push(func);
            continue;
        }
        if parser.accept_ident("func") {
            functions.push(parse_func(parser, None)?);
            continue;
        }
        return Err(ParseError::UnexpectedToken(parser.expect_ident()?));
    }

    Ok(functions)
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
    if parser.accept_ident("_") {
        return Ok(Vec::new());
    }

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
        "list" => parse_single_param(parser, Type::list),
        "option" => parse_single_param(parser, Type::option),
        "tuple" => parse_tuple(parser),
        "result" => parse_result(parser),
        _ => Ok(Type::Named(ident)),
    }
}

fn parse_single_param<F>(parser: &mut Parser, wrap: F) -> Result<Type, ParseError>
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
    let ok = parse_optional_type(parser)?;
    parser.expect_symbol(',')?;
    let err = parse_optional_type(parser)?;
    parser.expect_symbol('>')?;
    Ok(Type::result(ok, err))
}

fn parse_optional_type(parser: &mut Parser) -> Result<Option<Type>, ParseError> {
    if parser.accept_ident("_") {
        Ok(None)
    } else {
        Ok(Some(parse_type(parser)?))
    }
}

fn tokenize(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        if ch == '/' {
            chars.next();
            // Check for // line comment
            if matches!(chars.peek(), Some('/')) {
                while let Some(next) = chars.next() {
                    if next == '\n' {
                        break;
                    }
                }
                continue;
            }
            // Check for /* block comment */
            if matches!(chars.peek(), Some('*')) {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '*' && matches!(chars.peek(), Some('/')) {
                        chars.next();
                        break;
                    }
                }
                continue;
            }
            // Standalone / is a symbol (used in interface paths like wasi:cli/stdin)
            tokens.push(Token::Symbol('/'));
            continue;
        }

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
        '{' | '}' | '(' | ')' | '<' | '>' | ':' | ',' | '=' | ';' | '-'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variant_allows_recursive_reference() {
        let src = r#"
            variant node {
                leaf(s64),
                list(list<node>),
            }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.types.len(), 1);
    }

    #[test]
    fn parse_mutual_recursion() {
        let src = r#"
            variant expr { literal(lit) }
            variant lit { quoted(expr) }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.types.len(), 2);
    }

    #[test]
    fn parse_interface_with_functions() {
        let src = r#"
            interface api {
                variant node { leaf(s64), list(list<node>) }
                process: func(input: node) -> node
                ping: func() -> _
            }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.types.len(), 1);
        assert_eq!(interface.functions.len(), 2);
    }

    #[test]
    fn parse_result_and_tuple_types() {
        let src = r#"
            interface api {
                variant node { leaf(s64), list(list<node>) }
                transform: func(input: result<_, string>, data: tuple<s32, node>) -> result<node, string>
            }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.types.len(), 1);
        assert_eq!(interface.functions.len(), 1);
    }

    #[test]
    fn parse_type_alias_and_self_ref() {
        let src = r#"
            interface api {
                type node_id = u64
                variant node { leaf(node_id), next(self) }
                func wrap(id: node_id) -> node
            }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.types.len(), 2);
        assert_eq!(interface.functions.len(), 1);
    }

    #[test]
    fn parse_imports_and_exports() {
        let src = r#"
            variant node { leaf(s64) }
            import host {
                log: func(msg: string)
            }
            export api {
                process: func(input: node) -> node
            }
        "#;

        let interface = parse_interface(src).expect("parse");
        assert_eq!(interface.imports.len(), 1);
        assert_eq!(interface.exports.len(), 1);
        assert_eq!(interface.types.len(), 1);
    }

    // ========================================================================
    // World parsing tests
    // ========================================================================

    #[test]
    fn parse_world_basic() {
        let src = r#"
            world my-component {
                import logging
                export run: func() -> string
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.name, "my-component");
        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.exports.len(), 1);

        // Check import is a simple interface reference
        match &world.imports[0] {
            WorldItem::InterfacePath(path) => {
                assert_eq!(path.interface, "logging");
                assert!(path.namespace.is_none());
            }
            _ => panic!("Expected InterfacePath"),
        }

        // Check export is a function
        match &world.exports[0] {
            WorldItem::Function(func) => {
                assert_eq!(func.name, "run");
                assert_eq!(func.results.len(), 1);
            }
            _ => panic!("Expected Function"),
        }
    }

    #[test]
    fn parse_world_with_namespaced_imports() {
        let src = r#"
            world wasi-cli {
                import wasi:cli/stdin
                import wasi:cli/stdout
                import wasi:filesystem/types
                export main: func() -> result<_, string>
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.name, "wasi-cli");
        assert_eq!(world.imports.len(), 3);
        assert_eq!(world.exports.len(), 1);

        // Check first import has namespace
        match &world.imports[0] {
            WorldItem::InterfacePath(path) => {
                assert_eq!(path.namespace, Some("wasi".to_string()));
                assert_eq!(path.package, Some("cli".to_string()));
                assert_eq!(path.interface, "stdin");
            }
            _ => panic!("Expected InterfacePath"),
        }
    }

    #[test]
    fn parse_world_with_inline_interface() {
        let src = r#"
            world my-app {
                import host {
                    log: func(msg: string)
                    get-time: func() -> u64
                }
                export api {
                    process: func(input: string) -> string
                }
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.exports.len(), 1);

        // Check import is inline interface
        match &world.imports[0] {
            WorldItem::InlineInterface { name, functions } => {
                assert_eq!(name, "host");
                assert_eq!(functions.len(), 2);
            }
            _ => panic!("Expected InlineInterface"),
        }
    }

    #[test]
    fn parse_world_mixed_items() {
        let src = r#"
            world theater-actor {
                // Namespaced interface imports
                import wasi:io/streams

                // Standalone function imports
                import send-message: func(target: string, msg: string) -> result<_, string>

                // Inline interface import
                import runtime {
                    get-actor-id: func() -> string
                }

                // Function export
                export handle: func(msg: string) -> string
            }
        "#;

        let world = parse_world(src).expect("parse");
        assert_eq!(world.imports.len(), 3);
        assert_eq!(world.exports.len(), 1);
    }
}
