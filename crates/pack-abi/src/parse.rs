//! Value literal parser.
//!
//! Parses the text format produced by `Value`'s `Display` impl back into a `Value`.
//! This enables lossless round-tripping: `Display` → `FromStr` → `Display`.
//!
//! Grammar (informal):
//! ```text
//! value     = bool | number | char | string | tuple | list
//!           | option | result | record | variant | flags
//! bool      = "true" | "false"
//! number    = ["-"] digits ["." digits] suffix
//! suffix    = "u8" | "u16" | "u32" | "u64" | "s8" | "s16" | "s32" | "s64" | "f32" | "f64"
//! char      = "'" (escape | any) "'"
//! string    = '"' (escape | any)* '"'
//! escape    = "\\" | "\"" | "\n" | "\r" | "\t" | "\'"
//! tuple     = "(" [value ("," value)* [","]] ")"
//! list      = "[" [value ("," value)* [","]] "]"
//! option    = "some(" value ")" | "none"
//! result    = "ok(" value ")" | "err(" value ")"
//! flags     = "flags(0x" hex+ ")"
//! record    = [ident] "{" [field ("," field)* [","]] "}"
//! field     = ident ":" value
//! variant   = [ident] "::" ident ["(" [value ("," value)*] ")"]
//! ident     = [a-zA-Z_] [a-zA-Z0-9_-]*
//! ```

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::value::{Value, ValueType};

/// Parse error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "parse error at {}: {}", self.position, self.message)
    }
}

/// Parser state: wraps a string slice and current position.
struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.advance(c.len_utf8());
            } else {
                break;
            }
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), ParseError> {
        match self.peek() {
            Some(c) if c == expected => {
                self.advance(c.len_utf8());
                Ok(())
            }
            Some(c) => Err(self.error(alloc::format!("expected '{}', got '{}'", expected, c))),
            None => Err(self.error(alloc::format!("expected '{}', got EOF", expected))),
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.remaining().starts_with(s)
    }

    fn error(&self, message: String) -> ParseError {
        ParseError {
            message,
            position: self.pos,
        }
    }

    /// Parse a complete value.
    fn parse_value(&mut self) -> Result<Value, ParseError> {
        self.skip_whitespace();

        if self.is_eof() {
            return Err(self.error(String::from("unexpected EOF")));
        }

        match self.peek().unwrap() {
            '"' => self.parse_string(),
            '\'' => self.parse_char(),
            '(' => self.parse_tuple(),
            '[' => self.parse_list(),
            '{' => self.parse_record_body(String::new()),
            c if c == '-' || c.is_ascii_digit() => self.parse_number(),
            _ => self.parse_keyword_or_named(),
        }
    }

    fn parse_string(&mut self) -> Result<Value, ParseError> {
        self.expect_char('"')?;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error(String::from("unterminated string"))),
                Some('"') => {
                    self.advance(1);
                    return Ok(Value::String(s));
                }
                Some('\\') => {
                    self.advance(1);
                    match self.peek() {
                        Some('n') => {
                            self.advance(1);
                            s.push('\n');
                        }
                        Some('r') => {
                            self.advance(1);
                            s.push('\r');
                        }
                        Some('t') => {
                            self.advance(1);
                            s.push('\t');
                        }
                        Some('\\') => {
                            self.advance(1);
                            s.push('\\');
                        }
                        Some('"') => {
                            self.advance(1);
                            s.push('"');
                        }
                        Some(c) => {
                            return Err(self.error(alloc::format!("unknown escape '\\{}'", c)))
                        }
                        None => return Err(self.error(String::from("unterminated escape"))),
                    }
                }
                Some(c) => {
                    self.advance(c.len_utf8());
                    s.push(c);
                }
            }
        }
    }

    fn parse_char(&mut self) -> Result<Value, ParseError> {
        self.expect_char('\'')?;
        let c = match self.peek() {
            None => return Err(self.error(String::from("unterminated char"))),
            Some('\\') => {
                self.advance(1);
                match self.peek() {
                    Some('n') => {
                        self.advance(1);
                        '\n'
                    }
                    Some('r') => {
                        self.advance(1);
                        '\r'
                    }
                    Some('t') => {
                        self.advance(1);
                        '\t'
                    }
                    Some('\\') => {
                        self.advance(1);
                        '\\'
                    }
                    Some('\'') => {
                        self.advance(1);
                        '\''
                    }
                    Some(c) => {
                        return Err(self.error(alloc::format!("unknown char escape '\\{}'", c)))
                    }
                    None => return Err(self.error(String::from("unterminated char escape"))),
                }
            }
            Some(c) => {
                self.advance(c.len_utf8());
                c
            }
        };
        self.expect_char('\'')?;
        Ok(Value::Char(c))
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        // Consume optional minus
        if self.peek() == Some('-') {
            self.advance(1);
        }
        // Consume digits
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance(1);
            } else {
                break;
            }
        }
        // Check for decimal point
        let has_dot = if self.peek() == Some('.') {
            self.advance(1);
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance(1);
                } else {
                    break;
                }
            }
            true
        } else {
            false
        };

        let num_str = &self.input[start..self.pos];

        // Parse type suffix
        let remaining = self.remaining();
        let suffixes: &[(&str, u8)] = &[
            ("u8", 1),
            ("u16", 2),
            ("u32", 3),
            ("u64", 4),
            ("s8", 5),
            ("s16", 6),
            ("s32", 7),
            ("s64", 8),
            ("f32", 9),
            ("f64", 10),
        ];

        for &(suffix, id) in suffixes {
            if remaining.starts_with(suffix) {
                // Make sure suffix isn't followed by alphanumeric (e.g. "u320" shouldn't match "u32")
                let after = remaining
                    .get(suffix.len()..suffix.len() + 1)
                    .and_then(|s| s.chars().next());
                if after.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                    self.advance(suffix.len());
                    return self.make_number(num_str, id, has_dot);
                }
            }
        }

        Err(self.error(alloc::format!(
            "number '{}' missing type suffix (e.g. u32, s64, f32)",
            num_str
        )))
    }

    fn make_number(
        &self,
        num_str: &str,
        suffix_id: u8,
        _has_dot: bool,
    ) -> Result<Value, ParseError> {
        let pos = self.pos;
        let err = |msg: String| ParseError {
            message: msg,
            position: pos,
        };

        match suffix_id {
            1 => {
                // u8
                let v: u8 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::U8(v))
            }
            2 => {
                // u16
                let v: u16 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::U16(v))
            }
            3 => {
                // u32
                let v: u32 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::U32(v))
            }
            4 => {
                // u64
                let v: u64 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::U64(v))
            }
            5 => {
                // s8
                let v: i8 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::S8(v))
            }
            6 => {
                // s16
                let v: i16 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::S16(v))
            }
            7 => {
                // s32
                let v: i32 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::S32(v))
            }
            8 => {
                // s64
                let v: i64 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::S64(v))
            }
            9 => {
                // f32
                let v: f32 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::F32(v))
            }
            10 => {
                // f64
                let v: f64 = num_str.parse().map_err(|e| err(alloc::format!("{}", e)))?;
                Ok(Value::F64(v))
            }
            _ => Err(err(String::from("invalid suffix id"))),
        }
    }

    fn parse_tuple(&mut self) -> Result<Value, ParseError> {
        self.expect_char('(')?;
        self.skip_whitespace();
        if self.peek() == Some(')') {
            self.advance(1);
            return Ok(Value::Tuple(Vec::new()));
        }
        let items = self.parse_comma_list(')')?;
        Ok(Value::Tuple(items))
    }

    fn parse_list(&mut self) -> Result<Value, ParseError> {
        self.expect_char('[')?;
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.advance(1);
            return Ok(Value::List {
                elem_type: ValueType::S32, // default for empty list
                items: Vec::new(),
            });
        }
        let items = self.parse_comma_list(']')?;
        let elem_type = items
            .first()
            .map(|v| v.infer_type())
            .unwrap_or(ValueType::S32);
        Ok(Value::List { elem_type, items })
    }

    /// Parse comma-separated values until `end_char`.
    fn parse_comma_list(&mut self, end_char: char) -> Result<Vec<Value>, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some(end_char) {
                self.advance(1);
                return Ok(items);
            }
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance(1);
                }
                Some(c) if c == end_char => {}
                Some(c) => {
                    return Err(self.error(alloc::format!(
                        "expected ',' or '{}', got '{}'",
                        end_char,
                        c
                    )))
                }
                None => return Err(self.error(alloc::format!("expected '{}', got EOF", end_char))),
            }
        }
    }

    /// Parse keywords (true, false, none, some, ok, err, flags) or named things (records, variants).
    fn parse_keyword_or_named(&mut self) -> Result<Value, ParseError> {
        // Check for keywords first
        if self.starts_with("true") && !self.is_ident_continue_at(4) {
            self.advance(4);
            return Ok(Value::Bool(true));
        }
        if self.starts_with("false") && !self.is_ident_continue_at(5) {
            self.advance(5);
            return Ok(Value::Bool(false));
        }
        if self.starts_with("none") && !self.is_ident_continue_at(4) {
            self.advance(4);
            return Ok(Value::Option {
                inner_type: ValueType::S32,
                value: None,
            });
        }
        if self.starts_with("some(") {
            self.advance(5);
            let inner = self.parse_value()?;
            self.skip_whitespace();
            self.expect_char(')')?;
            let inner_type = inner.infer_type();
            return Ok(Value::Option {
                inner_type,
                value: Some(Box::new(inner)),
            });
        }
        if self.starts_with("ok(") {
            self.advance(3);
            let inner = self.parse_value()?;
            self.skip_whitespace();
            self.expect_char(')')?;
            let ok_type = inner.infer_type();
            return Ok(Value::Result {
                ok_type,
                err_type: ValueType::String,
                value: Ok(Box::new(inner)),
            });
        }
        if self.starts_with("err(") {
            self.advance(4);
            let inner = self.parse_value()?;
            self.skip_whitespace();
            self.expect_char(')')?;
            let err_type = inner.infer_type();
            return Ok(Value::Result {
                ok_type: ValueType::S32,
                err_type,
                value: Err(Box::new(inner)),
            });
        }
        if self.starts_with("flags(0x") {
            self.advance(8);
            let start = self.pos;
            while let Some(c) = self.peek() {
                if c.is_ascii_hexdigit() {
                    self.advance(1);
                } else {
                    break;
                }
            }
            let hex_str = &self.input[start..self.pos];
            let v = u64::from_str_radix(hex_str, 16)
                .map_err(|e| self.error(alloc::format!("invalid flags hex: {}", e)))?;
            self.expect_char(')')?;
            return Ok(Value::Flags(v));
        }

        // Must be an identifier — could be record or variant
        // Also handle leading :: for empty-type-name variants
        if self.starts_with("::") {
            self.advance(2);
            let case_name = self.parse_ident()?;
            let payload = self.parse_optional_payload()?;
            return Ok(Value::Variant {
                type_name: String::new(),
                case_name,
                tag: 0,
                payload,
            });
        }

        let ident = self.parse_ident()?;
        self.skip_whitespace();

        match self.peek() {
            Some('{') => self.parse_record_body(ident),
            Some(':') if self.starts_with("::") => {
                self.advance(2);
                let case_name = self.parse_ident()?;
                let payload = self.parse_optional_payload()?;
                Ok(Value::Variant {
                    type_name: ident,
                    case_name,
                    tag: 0,
                    payload,
                })
            }
            _ => Err(self.error(alloc::format!(
                "unexpected identifier '{}' (expected {{ or ::)",
                ident
            ))),
        }
    }

    fn parse_record_body(&mut self, type_name: String) -> Result<Value, ParseError> {
        self.expect_char('{')?;
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.advance(1);
            return Ok(Value::Record {
                type_name,
                fields: Vec::new(),
            });
        }
        let mut fields = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some('}') {
                self.advance(1);
                return Ok(Value::Record { type_name, fields });
            }
            let name = self.parse_ident()?;
            self.skip_whitespace();
            self.expect_char(':')?;
            self.skip_whitespace();
            let value = self.parse_value()?;
            fields.push((name, value));
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.advance(1);
                }
                Some('}') => {}
                Some(c) => {
                    return Err(self.error(alloc::format!("expected ',' or '}}', got '{}'", c)))
                }
                None => return Err(self.error(String::from("expected '}', got EOF"))),
            }
        }
    }

    fn parse_optional_payload(&mut self) -> Result<Vec<Value>, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some('(') {
            self.advance(1);
            self.skip_whitespace();
            if self.peek() == Some(')') {
                self.advance(1);
                return Ok(Vec::new());
            }
            let items = self.parse_comma_list(')')?;
            Ok(items)
        } else {
            Ok(Vec::new())
        }
    }

    fn parse_ident(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                self.advance(c.len_utf8());
            }
            _ => return Err(self.error(String::from("expected identifier"))),
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.advance(c.len_utf8());
            } else {
                break;
            }
        }
        Ok(String::from(&self.input[start..self.pos]))
    }

    /// Check if there's an identifier-continue character at offset from current position.
    fn is_ident_continue_at(&self, offset: usize) -> bool {
        self.remaining()
            .get(offset..offset + 1)
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }
}

/// Parse a value literal string into a `Value`.
pub fn parse_value(input: &str) -> Result<Value, ParseError> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if !parser.is_eof() {
        return Err(parser.error(alloc::format!("trailing input: '{}'", parser.remaining())));
    }
    Ok(value)
}

impl core::str::FromStr for Value {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_value(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn test_bool() {
        assert_eq!(parse_value("true").unwrap(), Value::Bool(true));
        assert_eq!(parse_value("false").unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_numbers() {
        assert_eq!(parse_value("42u8").unwrap(), Value::U8(42));
        assert_eq!(parse_value("1000u16").unwrap(), Value::U16(1000));
        assert_eq!(parse_value("123456u32").unwrap(), Value::U32(123456));
        assert_eq!(parse_value("99u64").unwrap(), Value::U64(99));
        assert_eq!(parse_value("-5s8").unwrap(), Value::S8(-5));
        assert_eq!(parse_value("-100s16").unwrap(), Value::S16(-100));
        assert_eq!(parse_value("0s32").unwrap(), Value::S32(0));
        assert_eq!(parse_value("-1s64").unwrap(), Value::S64(-1));
        assert_eq!(parse_value("3.14f32").unwrap(), Value::F32(3.14));
        assert_eq!(parse_value("2.718f64").unwrap(), Value::F64(2.718));
        assert_eq!(parse_value("1.0f32").unwrap(), Value::F32(1.0));
    }

    #[test]
    fn test_string() {
        assert_eq!(
            parse_value("\"hello\"").unwrap(),
            Value::String(String::from("hello"))
        );
        assert_eq!(
            parse_value("\"a\\nb\"").unwrap(),
            Value::String(String::from("a\nb"))
        );
        assert_eq!(
            parse_value("\"a\\\"b\"").unwrap(),
            Value::String(String::from("a\"b"))
        );
        assert_eq!(parse_value("\"\"").unwrap(), Value::String(String::new()));
    }

    #[test]
    fn test_char() {
        assert_eq!(parse_value("'x'").unwrap(), Value::Char('x'));
        assert_eq!(parse_value("'\\n'").unwrap(), Value::Char('\n'));
        assert_eq!(parse_value("'\\''").unwrap(), Value::Char('\''));
    }

    #[test]
    fn test_tuple() {
        assert_eq!(parse_value("()").unwrap(), Value::Tuple(Vec::new()));
        assert_eq!(
            parse_value("(42u32, \"hi\")").unwrap(),
            Value::Tuple(vec![Value::U32(42), Value::String(String::from("hi"))])
        );
    }

    #[test]
    fn test_list() {
        assert_eq!(
            parse_value("[]").unwrap(),
            Value::List {
                elem_type: ValueType::S32,
                items: Vec::new()
            }
        );
        assert_eq!(
            parse_value("[1u8, 2u8, 3u8]").unwrap(),
            Value::List {
                elem_type: ValueType::U8,
                items: vec![Value::U8(1), Value::U8(2), Value::U8(3)]
            }
        );
    }

    #[test]
    fn test_option() {
        assert_eq!(
            parse_value("none").unwrap(),
            Value::Option {
                inner_type: ValueType::S32,
                value: None
            }
        );
        assert_eq!(
            parse_value("some(42u32)").unwrap(),
            Value::Option {
                inner_type: ValueType::U32,
                value: Some(Box::new(Value::U32(42)))
            }
        );
    }

    #[test]
    fn test_result() {
        assert_eq!(
            parse_value("ok(1u32)").unwrap(),
            Value::Result {
                ok_type: ValueType::U32,
                err_type: ValueType::String,
                value: Ok(Box::new(Value::U32(1)))
            }
        );
        assert_eq!(
            parse_value("err(\"bad\")").unwrap(),
            Value::Result {
                ok_type: ValueType::S32,
                err_type: ValueType::String,
                value: Err(Box::new(Value::String(String::from("bad"))))
            }
        );
    }

    #[test]
    fn test_record() {
        let v = parse_value("actor-state{greeting: \"Hello\", count: 0u32}").unwrap();
        assert_eq!(
            v,
            Value::Record {
                type_name: String::from("actor-state"),
                fields: vec![
                    (
                        String::from("greeting"),
                        Value::String(String::from("Hello"))
                    ),
                    (String::from("count"), Value::U32(0)),
                ],
            }
        );
    }

    #[test]
    fn test_variant() {
        let v = parse_value("color::rgb(255u8, 0u8, 128u8)").unwrap();
        assert_eq!(
            v,
            Value::Variant {
                type_name: String::from("color"),
                case_name: String::from("rgb"),
                tag: 0,
                payload: vec![Value::U8(255), Value::U8(0), Value::U8(128)],
            }
        );

        // No payload
        let v = parse_value("status::active").unwrap();
        assert_eq!(
            v,
            Value::Variant {
                type_name: String::from("status"),
                case_name: String::from("active"),
                tag: 0,
                payload: Vec::new(),
            }
        );
    }

    #[test]
    fn test_variant_empty_type() {
        let v = parse_value("::my-case(1u32)").unwrap();
        assert_eq!(
            v,
            Value::Variant {
                type_name: String::new(),
                case_name: String::from("my-case"),
                tag: 0,
                payload: vec![Value::U32(1)],
            }
        );
    }

    #[test]
    fn test_flags() {
        assert_eq!(parse_value("flags(0xff)").unwrap(), Value::Flags(0xff));
        assert_eq!(parse_value("flags(0x0)").unwrap(), Value::Flags(0));
    }

    #[test]
    fn test_round_trip() {
        let values = vec![
            Value::Bool(true),
            Value::U32(42),
            Value::S64(-100),
            Value::F32(3.14),
            Value::String(String::from("hello \"world\"\nbye")),
            Value::Char('\t'),
            Value::Tuple(vec![Value::U8(1), Value::Bool(false)]),
            Value::List {
                elem_type: ValueType::S32,
                items: vec![Value::S32(1), Value::S32(2)],
            },
            Value::Option {
                inner_type: ValueType::String,
                value: Some(Box::new(Value::String(String::from("hi")))),
            },
            Value::Option {
                inner_type: ValueType::S32,
                value: None,
            },
            Value::Result {
                ok_type: ValueType::U32,
                err_type: ValueType::String,
                value: Ok(Box::new(Value::U32(99))),
            },
            Value::Record {
                type_name: String::from("point"),
                fields: vec![
                    (String::from("x"), Value::S32(10)),
                    (String::from("y"), Value::S32(20)),
                ],
            },
            Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("num"),
                tag: 1,
                payload: vec![Value::S64(42)],
            },
            Value::Flags(0xdeadbeef),
        ];

        for val in &values {
            let text = alloc::format!("{}", val);
            let parsed = parse_value(&text).unwrap_or_else(|e| {
                panic!("Failed to parse '{}': {}", text, e);
            });
            // Compare Display output (tag field may differ but Display should match)
            let reparsed_text = alloc::format!("{}", parsed);
            assert_eq!(text, reparsed_text, "Round-trip failed for: {}", text);
        }
    }

    #[test]
    fn test_nested_record() {
        let input = "outer{inner: point{x: 1s32, y: 2s32}, name: \"test\"}";
        let v = parse_value(input).unwrap();
        let output = alloc::format!("{}", v);
        assert_eq!(input, output);
    }

    #[test]
    fn test_trailing_comma() {
        // Trailing commas are accepted
        assert_eq!(
            parse_value("(1u32, 2u32,)").unwrap(),
            Value::Tuple(vec![Value::U32(1), Value::U32(2)])
        );
        assert_eq!(
            parse_value("[1u8,]").unwrap(),
            Value::List {
                elem_type: ValueType::U8,
                items: vec![Value::U8(1)]
            }
        );
    }
}
