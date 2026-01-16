//! Minimal WIT+ parser scaffold.
//!
//! Parses top-level type definitions and validates named references.

use super::{
    EnumDef, FlagsDef, Interface, ParseError, RecordDef, Type, TypeDef, VariantCase, VariantDef,
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
            if matches!(chars.peek(), Some('/')) {
                while let Some(next) = chars.next() {
                    if next == '\n' {
                        break;
                    }
                }
                continue;
            }
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
            return Err(ParseError::UnexpectedToken("/".to_string()));
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
}
