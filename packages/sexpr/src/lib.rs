//! S-expression evaluator package
//!
//! Demonstrates the pack-guest macros with a recursive type.
//! Evaluates simple Lisp-like expressions.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use composite_abi::ConversionError;
use pack_guest::{export, Value};

// Set up allocator and panic handler
pack_guest::setup_guest!();

// ============================================================================
// S-expression type
// ============================================================================

/// An S-expression - the core data type for Lisp-like languages
///
/// We implement From/TryFrom manually because Box<T> fields require special handling.
#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    /// A symbol (variable or function name)
    Sym(String),
    /// An integer number
    Num(i64),
    /// A floating point number
    Float(f64),
    /// A string literal
    Str(String),
    /// A boolean
    Bool(bool),
    /// Nil / empty list
    Nil,
    /// A cons cell (pair) - the building block of lists
    Cons(Box<SExpr>, Box<SExpr>),
    /// An error value
    Err(String),
}

// Manual From/TryFrom implementations for SExpr
impl From<SExpr> for Value {
    fn from(expr: SExpr) -> Value {
        match expr {
            SExpr::Sym(s) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("sym"),
                tag: 0,
                payload: alloc::vec![Value::String(s)],
            },
            SExpr::Num(n) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("num"),
                tag: 1,
                payload: alloc::vec![Value::S64(n)],
            },
            SExpr::Float(f) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("float"),
                tag: 2,
                payload: alloc::vec![Value::F64(f)],
            },
            SExpr::Str(s) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("str"),
                tag: 3,
                payload: alloc::vec![Value::String(s)],
            },
            SExpr::Bool(b) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("bool"),
                tag: 4,
                payload: alloc::vec![Value::Bool(b)],
            },
            SExpr::Nil => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("nil"),
                tag: 5,
                payload: alloc::vec![],
            },
            SExpr::Cons(head, tail) => {
                let head_val: Value = (*head).into();
                let tail_val: Value = (*tail).into();
                Value::Variant {
                    type_name: String::from("expr"),
                    case_name: String::from("cons"),
                    tag: 6,
                    payload: alloc::vec![Value::Tuple(alloc::vec![head_val, tail_val])],
                }
            }
            SExpr::Err(msg) => Value::Variant {
                type_name: String::from("expr"),
                case_name: String::from("err"),
                tag: 7,
                payload: alloc::vec![Value::String(msg)],
            },
        }
    }
}

impl TryFrom<Value> for SExpr {
    type Error = ConversionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Variant { tag, payload, .. } => match tag {
                0 => {
                    // Sym(String)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::String(s) => Ok(SExpr::Sym(s)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("String"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                1 => {
                    // Num(i64)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::S64(n) => Ok(SExpr::Num(n)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("S64"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                2 => {
                    // Float(f64)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::F64(f) => Ok(SExpr::Float(f)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("F64"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                3 => {
                    // Str(String)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::String(s) => Ok(SExpr::Str(s)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("String"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                4 => {
                    // Bool(bool)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::Bool(b) => Ok(SExpr::Bool(b)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("Bool"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                5 => {
                    // Nil
                    Ok(SExpr::Nil)
                }
                6 => {
                    // Cons(Box<SExpr>, Box<SExpr>)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::Tuple(items) if items.len() == 2 => {
                            let mut iter = items.into_iter();
                            let head = SExpr::try_from(iter.next().unwrap())?;
                            let tail = SExpr::try_from(iter.next().unwrap())?;
                            Ok(SExpr::Cons(Box::new(head), Box::new(tail)))
                        }
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("Tuple(2)"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                7 => {
                    // Err(String)
                    if payload.is_empty() {
                        return Err(ConversionError::MissingPayload);
                    }
                    match payload.into_iter().next().unwrap() {
                        Value::String(s) => Ok(SExpr::Err(s)),
                        other => Err(ConversionError::TypeMismatch {
                            expected: String::from("String"),
                            got: alloc::format!("{:?}", other),
                        }),
                    }
                }
                _ => Err(ConversionError::UnknownTag { tag, max: 8 }),
            },
            other => Err(ConversionError::ExpectedVariant(alloc::format!("{:?}", other))),
        }
    }
}

impl SExpr {
    /// Create a list from a vector of expressions
    pub fn list(items: Vec<SExpr>) -> SExpr {
        let mut result = SExpr::Nil;
        for item in items.into_iter().rev() {
            result = SExpr::Cons(Box::new(item), Box::new(result));
        }
        result
    }

    /// Convert a list to a vector (returns None if not a proper list)
    pub fn to_vec(&self) -> Option<Vec<SExpr>> {
        let mut result = Vec::new();
        let mut current = self;
        loop {
            match current {
                SExpr::Nil => return Some(result),
                SExpr::Cons(head, tail) => {
                    result.push((**head).clone());
                    current = tail;
                }
                _ => return None, // Improper list
            }
        }
    }

    /// Check if this is nil
    pub fn is_nil(&self) -> bool {
        matches!(self, SExpr::Nil)
    }

    /// Check if this is truthy (everything except Nil and Bool(false))
    pub fn is_truthy(&self) -> bool {
        !matches!(self, SExpr::Nil | SExpr::Bool(false))
    }
}

// ============================================================================
// Evaluator
// ============================================================================

/// Evaluate an S-expression
pub fn eval(expr: &SExpr) -> SExpr {
    match expr {
        // Self-evaluating forms
        SExpr::Num(_) | SExpr::Float(_) | SExpr::Str(_) | SExpr::Bool(_) | SExpr::Nil | SExpr::Err(_) => {
            expr.clone()
        }

        // Symbols evaluate to themselves for now (no environment)
        SExpr::Sym(_) => expr.clone(),

        // Function application
        SExpr::Cons(head, tail) => {
            if let SExpr::Sym(name) = &**head {
                let args = match tail.to_vec() {
                    Some(v) => v,
                    None => return SExpr::Err("invalid argument list".to_string()),
                };
                apply_builtin(name, &args)
            } else {
                SExpr::Err("first element must be a symbol".to_string())
            }
        }
    }
}

/// Apply a built-in function
fn apply_builtin(name: &str, args: &[SExpr]) -> SExpr {
    match name {
        // Arithmetic
        "+" | "add" => builtin_add(args),
        "-" | "sub" => builtin_sub(args),
        "*" | "mul" => builtin_mul(args),
        "/" | "div" => builtin_div(args),
        "%" | "mod" => builtin_mod(args),

        // Comparison
        "=" | "eq" => builtin_eq(args),
        "<" => builtin_lt(args),
        ">" => builtin_gt(args),
        "<=" => builtin_lte(args),
        ">=" => builtin_gte(args),

        // Logic
        "not" => builtin_not(args),
        "and" => builtin_and(args),
        "or" => builtin_or(args),

        // List operations
        "cons" => builtin_cons(args),
        "car" | "first" => builtin_car(args),
        "cdr" | "rest" => builtin_cdr(args),
        "list" => builtin_list(args),
        "len" | "length" => builtin_length(args),
        "nil?" => builtin_is_nil(args),

        // Control flow
        "if" => builtin_if(args),
        "quote" => builtin_quote(args),

        // Type predicates
        "num?" => builtin_is_num(args),
        "sym?" => builtin_is_sym(args),
        "str?" => builtin_is_str(args),
        "list?" => builtin_is_list(args),

        _ => SExpr::Err(alloc::format!("unknown function: {}", name)),
    }
}

// ============================================================================
// Built-in functions
// ============================================================================

fn builtin_add(args: &[SExpr]) -> SExpr {
    let mut sum: i64 = 0;
    let mut is_float = false;
    let mut float_sum: f64 = 0.0;

    for arg in args {
        let evaled = eval(arg);
        match evaled {
            SExpr::Num(n) => {
                if is_float {
                    float_sum += n as f64;
                } else {
                    sum += n;
                }
            }
            SExpr::Float(f) => {
                if !is_float {
                    is_float = true;
                    float_sum = sum as f64;
                }
                float_sum += f;
            }
            _ => return SExpr::Err("+ requires numbers".to_string()),
        }
    }

    if is_float {
        SExpr::Float(float_sum)
    } else {
        SExpr::Num(sum)
    }
}

fn builtin_sub(args: &[SExpr]) -> SExpr {
    if args.is_empty() {
        return SExpr::Err("- requires at least one argument".to_string());
    }

    let first = eval(&args[0]);
    if args.len() == 1 {
        // Unary negation
        return match first {
            SExpr::Num(n) => SExpr::Num(-n),
            SExpr::Float(f) => SExpr::Float(-f),
            _ => SExpr::Err("- requires numbers".to_string()),
        };
    }

    let mut result = match first {
        SExpr::Num(n) => n as f64,
        SExpr::Float(f) => f,
        _ => return SExpr::Err("- requires numbers".to_string()),
    };

    let mut is_float = matches!(first, SExpr::Float(_));

    for arg in &args[1..] {
        let evaled = eval(arg);
        match evaled {
            SExpr::Num(n) => result -= n as f64,
            SExpr::Float(f) => {
                is_float = true;
                result -= f;
            }
            _ => return SExpr::Err("- requires numbers".to_string()),
        }
    }

    if is_float {
        SExpr::Float(result)
    } else {
        SExpr::Num(result as i64)
    }
}

fn builtin_mul(args: &[SExpr]) -> SExpr {
    let mut product: i64 = 1;
    let mut is_float = false;
    let mut float_product: f64 = 1.0;

    for arg in args {
        let evaled = eval(arg);
        match evaled {
            SExpr::Num(n) => {
                if is_float {
                    float_product *= n as f64;
                } else {
                    product *= n;
                }
            }
            SExpr::Float(f) => {
                if !is_float {
                    is_float = true;
                    float_product = product as f64;
                }
                float_product *= f;
            }
            _ => return SExpr::Err("* requires numbers".to_string()),
        }
    }

    if is_float {
        SExpr::Float(float_product)
    } else {
        SExpr::Num(product)
    }
}

fn builtin_div(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("/ requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => {
            if *y == 0 {
                SExpr::Err("division by zero".to_string())
            } else {
                SExpr::Num(x / y)
            }
        }
        (SExpr::Num(x), SExpr::Float(y)) => {
            if *y == 0.0 {
                SExpr::Err("division by zero".to_string())
            } else {
                SExpr::Float(*x as f64 / y)
            }
        }
        (SExpr::Float(x), SExpr::Num(y)) => {
            if *y == 0 {
                SExpr::Err("division by zero".to_string())
            } else {
                SExpr::Float(x / *y as f64)
            }
        }
        (SExpr::Float(x), SExpr::Float(y)) => {
            if *y == 0.0 {
                SExpr::Err("division by zero".to_string())
            } else {
                SExpr::Float(x / y)
            }
        }
        _ => SExpr::Err("/ requires numbers".to_string()),
    }
}

fn builtin_mod(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("% requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => {
            if *y == 0 {
                SExpr::Err("modulo by zero".to_string())
            } else {
                SExpr::Num(x % y)
            }
        }
        _ => SExpr::Err("% requires integers".to_string()),
    }
}

fn builtin_eq(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("= requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    SExpr::Bool(a == b)
}

fn builtin_lt(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("< requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => SExpr::Bool(x < y),
        (SExpr::Float(x), SExpr::Float(y)) => SExpr::Bool(x < y),
        (SExpr::Num(x), SExpr::Float(y)) => SExpr::Bool((*x as f64) < *y),
        (SExpr::Float(x), SExpr::Num(y)) => SExpr::Bool(*x < (*y as f64)),
        _ => SExpr::Err("< requires numbers".to_string()),
    }
}

fn builtin_gt(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("> requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => SExpr::Bool(x > y),
        (SExpr::Float(x), SExpr::Float(y)) => SExpr::Bool(x > y),
        (SExpr::Num(x), SExpr::Float(y)) => SExpr::Bool((*x as f64) > *y),
        (SExpr::Float(x), SExpr::Num(y)) => SExpr::Bool(*x > (*y as f64)),
        _ => SExpr::Err("> requires numbers".to_string()),
    }
}

fn builtin_lte(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("<= requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => SExpr::Bool(x <= y),
        (SExpr::Float(x), SExpr::Float(y)) => SExpr::Bool(x <= y),
        (SExpr::Num(x), SExpr::Float(y)) => SExpr::Bool((*x as f64) <= *y),
        (SExpr::Float(x), SExpr::Num(y)) => SExpr::Bool(*x <= (*y as f64)),
        _ => SExpr::Err("<= requires numbers".to_string()),
    }
}

fn builtin_gte(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err(">= requires exactly 2 arguments".to_string());
    }

    let a = eval(&args[0]);
    let b = eval(&args[1]);

    match (&a, &b) {
        (SExpr::Num(x), SExpr::Num(y)) => SExpr::Bool(x >= y),
        (SExpr::Float(x), SExpr::Float(y)) => SExpr::Bool(x >= y),
        (SExpr::Num(x), SExpr::Float(y)) => SExpr::Bool((*x as f64) >= *y),
        (SExpr::Float(x), SExpr::Num(y)) => SExpr::Bool(*x >= (*y as f64)),
        _ => SExpr::Err(">= requires numbers".to_string()),
    }
}

fn builtin_not(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("not requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(!val.is_truthy())
}

fn builtin_and(args: &[SExpr]) -> SExpr {
    for arg in args {
        let val = eval(arg);
        if !val.is_truthy() {
            return SExpr::Bool(false);
        }
    }
    SExpr::Bool(true)
}

fn builtin_or(args: &[SExpr]) -> SExpr {
    for arg in args {
        let val = eval(arg);
        if val.is_truthy() {
            return SExpr::Bool(true);
        }
    }
    SExpr::Bool(false)
}

fn builtin_cons(args: &[SExpr]) -> SExpr {
    if args.len() != 2 {
        return SExpr::Err("cons requires exactly 2 arguments".to_string());
    }

    let head = eval(&args[0]);
    let tail = eval(&args[1]);
    SExpr::Cons(Box::new(head), Box::new(tail))
}

fn builtin_car(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("car requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    match val {
        SExpr::Cons(head, _) => *head,
        SExpr::Nil => SExpr::Err("car of nil".to_string()),
        _ => SExpr::Err("car requires a cons cell".to_string()),
    }
}

fn builtin_cdr(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("cdr requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    match val {
        SExpr::Cons(_, tail) => *tail,
        SExpr::Nil => SExpr::Err("cdr of nil".to_string()),
        _ => SExpr::Err("cdr requires a cons cell".to_string()),
    }
}

fn builtin_list(args: &[SExpr]) -> SExpr {
    let evaled: Vec<_> = args.iter().map(eval).collect();
    SExpr::list(evaled)
}

fn builtin_length(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("length requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    match val.to_vec() {
        Some(v) => SExpr::Num(v.len() as i64),
        None => SExpr::Err("length requires a proper list".to_string()),
    }
}

fn builtin_is_nil(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("nil? requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(val.is_nil())
}

fn builtin_if(args: &[SExpr]) -> SExpr {
    if args.len() < 2 || args.len() > 3 {
        return SExpr::Err("if requires 2 or 3 arguments".to_string());
    }

    let cond = eval(&args[0]);
    if cond.is_truthy() {
        eval(&args[1])
    } else if args.len() == 3 {
        eval(&args[2])
    } else {
        SExpr::Nil
    }
}

fn builtin_quote(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("quote requires exactly 1 argument".to_string());
    }

    args[0].clone()
}

fn builtin_is_num(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("num? requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(matches!(val, SExpr::Num(_) | SExpr::Float(_)))
}

fn builtin_is_sym(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("sym? requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(matches!(val, SExpr::Sym(_)))
}

fn builtin_is_str(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("str? requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(matches!(val, SExpr::Str(_)))
}

fn builtin_is_list(args: &[SExpr]) -> SExpr {
    if args.len() != 1 {
        return SExpr::Err("list? requires exactly 1 argument".to_string());
    }

    let val = eval(&args[0]);
    SExpr::Bool(matches!(val, SExpr::Nil | SExpr::Cons(_, _)))
}

// ============================================================================
// WASM interface
// ============================================================================

/// Evaluate an S-expression
/// Input: encoded SExpr
/// Output: encoded SExpr (result of evaluation)
#[export]
fn evaluate(expr: SExpr) -> SExpr {
    eval(&expr)
}
