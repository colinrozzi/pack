//! Structural value matching — `Pattern`.
//!
//! A [`Pattern`] is a *partial value tree*: the same shape as the `Value` it
//! matches, but with holes. Matching is a co-recursive walk of the pattern tree
//! against the value tree — the same tree model interface matching uses on
//! *types*, generalized to carry data (`Equals`) and allow holes (`Any`,
//! `open`).
//!
//! # Semantics: exact by default, opt-in partial
//!
//! Every container matches **exactly** unless its `open` flag is set:
//!
//! - **Record** — exact: field set matches exactly. `open`: listed fields must
//!   be present and match; extra fields ignored.
//! - **Map** — exact: key set matches exactly. `open`: listed keys present and
//!   their values match; extra keys ignored.
//! - **Set** — exact: patterns ↔ elements one-to-one. `open`: each listed
//!   pattern matched by some distinct element; extras ignored (containment).
//! - **List** — exact: same length, positional. `open`: listed patterns match
//!   the leading positions (prefix); trailing extras ignored.
//! - **Tuple / Variant payload** — always exact; arity is pinned by the type.
//!
//! [`Pattern::Any`] matches any value (a whole-subtree hole); `open` is strictly
//! about *cardinality* (extra fields/keys/elements), so the two ideas stay
//! orthogonal.
//!
//! # Scope
//!
//! `Pattern` is a **pure structural matcher** — no `and`/`or`/`not`, no regex or
//! comparisons. A union is a `Vec<Pattern>` at the call site (match if any
//! matches). This keeps [`Pattern::matches`] total and cheap, and keeps a
//! pattern trivially serializable (via `From`/`TryFrom<Value>`) so a guest can
//! *send* one over the wire as a subscription filter.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::value::{Value, ValueType};
use crate::ConversionError;

/// A structural matcher over [`Value`] trees. See the module docs for semantics.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Pattern {
    /// Matches any value.
    Any,
    /// Matches a value exactly equal to this one.
    Equals(Value),
    /// Matches a record. Exact field set unless `open`.
    Record {
        fields: Vec<(String, Pattern)>,
        open: bool,
    },
    /// Matches a map by key. Exact key set unless `open`. Keys are concrete
    /// values; each listed key's value must match its pattern.
    Map {
        entries: Vec<(Value, Pattern)>,
        open: bool,
    },
    /// Matches a set. Patterns match distinct elements one-to-one; `open` allows
    /// extra elements (containment).
    Set { items: Vec<Pattern>, open: bool },
    /// Matches a list positionally. Exact length unless `open` (then the
    /// patterns match a leading prefix).
    List { items: Vec<Pattern>, open: bool },
    /// Matches a tuple positionally (always exact — arity is fixed).
    Tuple(Vec<Pattern>),
    /// Matches a variant: the case name must equal, and the payload matches
    /// positionally (always exact).
    Variant {
        case_name: String,
        payload: Vec<Pattern>,
    },
    /// Matches `some(x)` where `x` matches the inner pattern.
    Some(Box<Pattern>),
    /// Matches `none`.
    None,
    /// Matches `ok(x)` where `x` matches the inner pattern.
    Ok(Box<Pattern>),
    /// Matches `err(x)` where `x` matches the inner pattern.
    Err(Box<Pattern>),
}

impl Pattern {
    /// Does `value` match this pattern? A co-recursive walk of the pattern tree
    /// against the value tree.
    pub fn matches(&self, value: &Value) -> bool {
        match self {
            Pattern::Any => true,
            Pattern::Equals(expected) => value == expected,

            Pattern::Record { fields, open } => match value {
                Value::Record {
                    fields: vfields, ..
                } => {
                    if !open && vfields.len() != fields.len() {
                        return false;
                    }
                    fields.iter().all(|(name, pat)| {
                        vfields
                            .iter()
                            .find(|(n, _)| n == name)
                            .is_some_and(|(_, v)| pat.matches(v))
                    })
                }
                _ => false,
            },

            Pattern::Map { entries, open } => match value {
                Value::Map {
                    entries: ventries, ..
                } => {
                    if !open && ventries.len() != entries.len() {
                        return false;
                    }
                    entries.iter().all(|(key, pat)| {
                        ventries
                            .iter()
                            .find(|(vk, _)| vk == key)
                            .is_some_and(|(_, vv)| pat.matches(vv))
                    })
                }
                _ => false,
            },

            Pattern::Set { items, open } => match value {
                Value::Set { items: vitems, .. } => set_matches(items, vitems, *open),
                _ => false,
            },

            Pattern::List { items, open } => match value {
                Value::List { items: vitems, .. } => {
                    if *open {
                        vitems.len() >= items.len()
                            && items.iter().zip(vitems).all(|(p, v)| p.matches(v))
                    } else {
                        vitems.len() == items.len()
                            && items.iter().zip(vitems).all(|(p, v)| p.matches(v))
                    }
                }
                _ => false,
            },

            Pattern::Tuple(items) => match value {
                Value::Tuple(vitems) => {
                    vitems.len() == items.len()
                        && items.iter().zip(vitems).all(|(p, v)| p.matches(v))
                }
                _ => false,
            },

            Pattern::Variant { case_name, payload } => match value {
                Value::Variant {
                    case_name: vcase,
                    payload: vpayload,
                    ..
                } => {
                    vcase == case_name
                        && vpayload.len() == payload.len()
                        && payload.iter().zip(vpayload).all(|(p, v)| p.matches(v))
                }
                _ => false,
            },

            Pattern::Some(inner) => match value {
                Value::Option {
                    value: core::option::Option::Some(v),
                    ..
                } => inner.matches(v),
                _ => false,
            },
            Pattern::None => matches!(
                value,
                Value::Option {
                    value: core::option::Option::None,
                    ..
                }
            ),
            Pattern::Ok(inner) => match value {
                Value::Result {
                    value: core::result::Result::Ok(v),
                    ..
                } => inner.matches(v),
                _ => false,
            },
            Pattern::Err(inner) => match value {
                Value::Result {
                    value: core::result::Result::Err(v),
                    ..
                } => inner.matches(v),
                _ => false,
            },
        }
    }

    // ---- ergonomic constructors -------------------------------------------

    /// [`Pattern::Any`].
    pub fn any() -> Self {
        Pattern::Any
    }
    /// [`Pattern::Equals`] from anything convertible to a [`Value`].
    pub fn equals(v: impl Into<Value>) -> Self {
        Pattern::Equals(v.into())
    }
    /// An exact record pattern.
    pub fn record(fields: impl IntoIterator<Item = (impl Into<String>, Pattern)>) -> Self {
        Pattern::Record {
            fields: fields.into_iter().map(|(n, p)| (n.into(), p)).collect(),
            open: false,
        }
    }
    /// An open record pattern (extra fields ignored).
    pub fn record_open(fields: impl IntoIterator<Item = (impl Into<String>, Pattern)>) -> Self {
        Pattern::Record {
            fields: fields.into_iter().map(|(n, p)| (n.into(), p)).collect(),
            open: true,
        }
    }
    /// An exact map pattern.
    pub fn map(entries: impl IntoIterator<Item = (impl Into<Value>, Pattern)>) -> Self {
        Pattern::Map {
            entries: entries.into_iter().map(|(k, p)| (k.into(), p)).collect(),
            open: false,
        }
    }
    /// An open map pattern (extra keys ignored).
    pub fn map_open(entries: impl IntoIterator<Item = (impl Into<Value>, Pattern)>) -> Self {
        Pattern::Map {
            entries: entries.into_iter().map(|(k, p)| (k.into(), p)).collect(),
            open: true,
        }
    }
    /// An exact set pattern.
    pub fn set(items: impl IntoIterator<Item = Pattern>) -> Self {
        Pattern::Set {
            items: items.into_iter().collect(),
            open: false,
        }
    }
    /// An open set pattern (containment — extra elements ignored).
    pub fn set_open(items: impl IntoIterator<Item = Pattern>) -> Self {
        Pattern::Set {
            items: items.into_iter().collect(),
            open: true,
        }
    }
    /// An exact list pattern (positional, same length).
    pub fn list(items: impl IntoIterator<Item = Pattern>) -> Self {
        Pattern::List {
            items: items.into_iter().collect(),
            open: false,
        }
    }
    /// An open list pattern (the patterns match a leading prefix).
    pub fn list_open(items: impl IntoIterator<Item = Pattern>) -> Self {
        Pattern::List {
            items: items.into_iter().collect(),
            open: true,
        }
    }
    /// A tuple pattern (positional, exact).
    pub fn tuple(items: impl IntoIterator<Item = Pattern>) -> Self {
        Pattern::Tuple(items.into_iter().collect())
    }
    /// A variant pattern.
    pub fn variant(
        case_name: impl Into<String>,
        payload: impl IntoIterator<Item = Pattern>,
    ) -> Self {
        Pattern::Variant {
            case_name: case_name.into(),
            payload: payload.into_iter().collect(),
        }
    }
    /// A `some(p)` pattern.
    pub fn some(p: Pattern) -> Self {
        Pattern::Some(Box::new(p))
    }
    /// A `none` pattern.
    pub fn none() -> Self {
        Pattern::None
    }
    /// An `ok(p)` pattern.
    pub fn ok(p: Pattern) -> Self {
        Pattern::Ok(Box::new(p))
    }
    /// An `err(p)` pattern.
    pub fn err(p: Pattern) -> Self {
        Pattern::Err(Box::new(p))
    }
}

/// Bipartite matching: every pattern must match a *distinct* element. For a
/// closed set the counts must also be equal (a perfect matching), so no element
/// is left unaccounted for. Uses Kuhn's augmenting-path algorithm.
fn set_matches(pats: &[Pattern], vals: &[Value], open: bool) -> bool {
    if !open && pats.len() != vals.len() {
        return false;
    }
    if pats.len() > vals.len() {
        return false;
    }
    let mut match_of: Vec<usize> = alloc::vec![usize::MAX; vals.len()];
    for i in 0..pats.len() {
        let mut seen = alloc::vec![false; vals.len()];
        if !augment(i, pats, vals, &mut seen, &mut match_of) {
            return false;
        }
    }
    true
}

fn augment(
    i: usize,
    pats: &[Pattern],
    vals: &[Value],
    seen: &mut [bool],
    match_of: &mut [usize],
) -> bool {
    for j in 0..vals.len() {
        if !seen[j] && pats[i].matches(&vals[j]) {
            seen[j] = true;
            if match_of[j] == usize::MAX || augment(match_of[j], pats, vals, seen, match_of) {
                match_of[j] = i;
                return true;
            }
        }
    }
    false
}

// ===========================================================================
// Wire marshaling: Pattern <-> Value
//
// A pattern serializes as a tagged `Value::Variant` so it can ride the graph
// ABI like any other value (a guest builds a Pattern, encodes it, and sends it
// as a subscription filter; the host decodes and matches). Recursion is handled
// naturally by nesting — no `Box` in the wire form.
// ===========================================================================

const TYPE_NAME: &str = "pattern";

const P_ANY: usize = 0;
const P_EQUALS: usize = 1;
const P_RECORD: usize = 2;
const P_MAP: usize = 3;
const P_SET: usize = 4;
const P_LIST: usize = 5;
const P_TUPLE: usize = 6;
const P_VARIANT: usize = 7;
const P_SOME: usize = 8;
const P_NONE: usize = 9;
const P_OK: usize = 10;
const P_ERR: usize = 11;

fn variant(case_name: &str, tag: usize, payload: Vec<Value>) -> Value {
    Value::Variant {
        type_name: String::from(TYPE_NAME),
        case_name: String::from(case_name),
        tag,
        payload,
    }
}

fn pat_list(pats: Vec<Pattern>) -> Value {
    Value::List {
        elem_type: ValueType::Variant(String::from(TYPE_NAME)),
        items: pats.into_iter().map(Value::from).collect(),
    }
}

fn open_flag(open: bool) -> Value {
    Value::Bool(open)
}

impl From<Pattern> for Value {
    fn from(p: Pattern) -> Value {
        match p {
            Pattern::Any => variant("any", P_ANY, Vec::new()),
            Pattern::Equals(v) => variant("equals", P_EQUALS, alloc::vec![v]),
            Pattern::Record { fields, open } => {
                let items = fields
                    .into_iter()
                    .map(|(n, p)| Value::Tuple(alloc::vec![Value::String(n), Value::from(p)]))
                    .collect();
                variant(
                    "record",
                    P_RECORD,
                    alloc::vec![
                        Value::List {
                            elem_type: ValueType::Tuple(alloc::vec![
                                ValueType::String,
                                ValueType::Variant(String::from(TYPE_NAME)),
                            ]),
                            items,
                        },
                        open_flag(open),
                    ],
                )
            }
            Pattern::Map { entries, open } => {
                let items = entries
                    .into_iter()
                    .map(|(k, p)| Value::Tuple(alloc::vec![k, Value::from(p)]))
                    .collect();
                variant(
                    "map",
                    P_MAP,
                    alloc::vec![
                        Value::List {
                            elem_type: ValueType::Tuple(alloc::vec![]),
                            items,
                        },
                        open_flag(open),
                    ],
                )
            }
            Pattern::Set { items, open } => {
                variant("set", P_SET, alloc::vec![pat_list(items), open_flag(open)])
            }
            Pattern::List { items, open } => variant(
                "list",
                P_LIST,
                alloc::vec![pat_list(items), open_flag(open)],
            ),
            Pattern::Tuple(items) => variant("tuple", P_TUPLE, alloc::vec![pat_list(items)]),
            Pattern::Variant { case_name, payload } => variant(
                "variant",
                P_VARIANT,
                alloc::vec![Value::String(case_name), pat_list(payload)],
            ),
            Pattern::Some(inner) => variant("some", P_SOME, alloc::vec![Value::from(*inner)]),
            Pattern::None => variant("none", P_NONE, Vec::new()),
            Pattern::Ok(inner) => variant("ok", P_OK, alloc::vec![Value::from(*inner)]),
            Pattern::Err(inner) => variant("err", P_ERR, alloc::vec![Value::from(*inner)]),
        }
    }
}

fn expected(what: &str, got: &Value) -> ConversionError {
    ConversionError::TypeMismatch {
        expected: String::from(what),
        got: alloc::format!("{:?}", got),
    }
}

fn take_pats(v: Value) -> Result<Vec<Pattern>, ConversionError> {
    match v {
        Value::List { items, .. } => items.into_iter().map(Pattern::try_from).collect(),
        other => Err(expected("list of patterns", &other)),
    }
}

fn take_bool(v: &Value) -> Result<bool, ConversionError> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => Err(expected("bool", other)),
    }
}

impl TryFrom<Value> for Pattern {
    type Error = ConversionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let (tag, mut payload) = match value {
            Value::Variant { tag, payload, .. } => (tag, payload),
            other => return Err(expected("pattern variant", &other)),
        };
        // Pull payload elements positionally.
        let mut next = move || payload.pop();
        match tag {
            P_ANY => Ok(Pattern::Any),
            P_NONE => Ok(Pattern::None),
            P_EQUALS => {
                let v = next().ok_or(ConversionError::MissingPayload)?;
                Ok(Pattern::Equals(v))
            }
            P_SOME => Ok(Pattern::Some(Box::new(Pattern::try_from(
                next().ok_or(ConversionError::MissingPayload)?,
            )?))),
            P_OK => Ok(Pattern::Ok(Box::new(Pattern::try_from(
                next().ok_or(ConversionError::MissingPayload)?,
            )?))),
            P_ERR => Ok(Pattern::Err(Box::new(Pattern::try_from(
                next().ok_or(ConversionError::MissingPayload)?,
            )?))),
            P_TUPLE => Ok(Pattern::Tuple(take_pats(
                next().ok_or(ConversionError::MissingPayload)?,
            )?)),
            P_SET | P_LIST => {
                // payload = [items_list, open_bool]; popped in reverse order.
                let open = take_bool(&next().ok_or(ConversionError::MissingPayload)?)?;
                let items = take_pats(next().ok_or(ConversionError::MissingPayload)?)?;
                Ok(if tag == P_SET {
                    Pattern::Set { items, open }
                } else {
                    Pattern::List { items, open }
                })
            }
            P_VARIANT => {
                // payload = [case_name, payload_list]; popped in reverse order.
                let pats = take_pats(next().ok_or(ConversionError::MissingPayload)?)?;
                let case_name = match next().ok_or(ConversionError::MissingPayload)? {
                    Value::String(s) => s,
                    other => return Err(expected("variant case name (string)", &other)),
                };
                Ok(Pattern::Variant {
                    case_name,
                    payload: pats,
                })
            }
            P_RECORD => {
                // payload = [fields_list, open_bool]; popped in reverse order.
                let open = take_bool(&next().ok_or(ConversionError::MissingPayload)?)?;
                let fields = match next().ok_or(ConversionError::MissingPayload)? {
                    Value::List { items, .. } => items
                        .into_iter()
                        .map(|it| match it {
                            Value::Tuple(mut kv) if kv.len() == 2 => {
                                let pat = Pattern::try_from(kv.pop().unwrap())?;
                                let name = match kv.pop().unwrap() {
                                    Value::String(s) => s,
                                    other => return Err(expected("record field name", &other)),
                                };
                                Ok((name, pat))
                            }
                            other => Err(expected("record field (name, pattern)", &other)),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    other => return Err(expected("record fields list", &other)),
                };
                Ok(Pattern::Record { fields, open })
            }
            P_MAP => {
                // payload = [entries_list, open_bool]; popped in reverse order.
                let open = take_bool(&next().ok_or(ConversionError::MissingPayload)?)?;
                let entries = match next().ok_or(ConversionError::MissingPayload)? {
                    Value::List { items, .. } => items
                        .into_iter()
                        .map(|it| match it {
                            Value::Tuple(mut kv) if kv.len() == 2 => {
                                let pat = Pattern::try_from(kv.pop().unwrap())?;
                                let key = kv.pop().unwrap();
                                Ok((key, pat))
                            }
                            other => Err(expected("map entry (key, pattern)", &other)),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    other => return Err(expected("map entries list", &other)),
                };
                Ok(Pattern::Map { entries, open })
            }
            other => Err(ConversionError::UnknownTag {
                tag: other,
                max: P_ERR,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::string::ToString;

    fn rec(fields: &[(&str, Value)]) -> Value {
        Value::Record {
            type_name: String::new(),
            fields: fields
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
        }
    }

    fn s(x: &str) -> Value {
        Value::String(x.to_string())
    }

    #[test]
    fn any_matches_anything() {
        assert!(Pattern::any().matches(&Value::U32(7)));
        assert!(Pattern::any().matches(&s("x")));
    }

    #[test]
    fn equals_is_exact() {
        assert!(Pattern::equals(1u32).matches(&Value::U32(1)));
        assert!(!Pattern::equals(1u32).matches(&Value::U32(2)));
        // type-sensitive: u32 != s32
        assert!(!Pattern::equals(1u32).matches(&Value::S32(1)));
    }

    #[test]
    fn record_exact_vs_open() {
        let v = rec(&[("kind", s("msg")), ("ts", Value::U64(9))]);

        // open: only the named field must match; extra fields ignored.
        assert!(Pattern::record_open([("kind", Pattern::equals("msg".to_string()))]).matches(&v));
        // exact: the extra `ts` field makes the field set differ -> no match.
        assert!(!Pattern::record([("kind", Pattern::equals("msg".to_string()))]).matches(&v));
        // exact with the full field set matches.
        assert!(Pattern::record([
            ("kind", Pattern::equals("msg".to_string())),
            ("ts", Pattern::any()),
        ])
        .matches(&v));
        // a listed field absent from the value never matches.
        assert!(!Pattern::record_open([("missing", Pattern::any())]).matches(&v));
    }

    #[test]
    fn map_matches_by_key() {
        let mut m = BTreeMap::new();
        m.insert("a".to_string(), 1u32);
        m.insert("b".to_string(), 2u32);
        let v: Value = m.into();

        // open: has key "a" whose value is 1; ignore "b".
        assert!(Pattern::map_open([(s("a"), Pattern::equals(1u32))]).matches(&v));
        // wrong value under the key.
        assert!(!Pattern::map_open([(s("a"), Pattern::equals(2u32))]).matches(&v));
        // exact needs the whole key set.
        assert!(!Pattern::map([(s("a"), Pattern::any())]).matches(&v));
        assert!(Pattern::map([(s("a"), Pattern::any()), (s("b"), Pattern::any())]).matches(&v));
    }

    #[test]
    fn set_containment_and_exact() {
        let mut set = BTreeSet::new();
        set.insert(1u32);
        set.insert(2u32);
        set.insert(3u32);
        let v: Value = set.into();

        // open: contains an element == 2 (extras ignored).
        assert!(Pattern::set_open([Pattern::equals(2u32)]).matches(&v));
        // open: two distinct elements, one ==1 and one ==3.
        assert!(Pattern::set_open([Pattern::equals(1u32), Pattern::equals(3u32)]).matches(&v));
        // exact needs a 1:1 pattern<->element cover.
        assert!(!Pattern::set([Pattern::equals(1u32)]).matches(&v));
        assert!(Pattern::set([
            Pattern::equals(1u32),
            Pattern::equals(2u32),
            Pattern::equals(3u32),
        ])
        .matches(&v));
    }

    #[test]
    fn set_needs_distinct_elements() {
        let mut set = BTreeSet::new();
        set.insert(5u32);
        let v: Value = set.into(); // {5}

        // Two patterns both want a match, but there's only one element -> the
        // bijection fails even though each pattern individually matches.
        assert!(!Pattern::set_open([Pattern::equals(5u32), Pattern::equals(5u32)]).matches(&v));
        // One pattern is fine.
        assert!(Pattern::set_open([Pattern::equals(5u32)]).matches(&v));
    }

    #[test]
    fn list_positional_exact_and_prefix() {
        let v = Value::List {
            elem_type: ValueType::U32,
            items: alloc::vec![Value::U32(1), Value::U32(2), Value::U32(3)],
        };
        // exact: same length, positional.
        assert!(
            Pattern::list([Pattern::equals(1u32), Pattern::any(), Pattern::equals(3u32),])
                .matches(&v)
        );
        // exact wrong length.
        assert!(!Pattern::list([Pattern::equals(1u32)]).matches(&v));
        // open: prefix.
        assert!(Pattern::list_open([Pattern::equals(1u32), Pattern::equals(2u32)]).matches(&v));
        // open prefix mismatch.
        assert!(!Pattern::list_open([Pattern::equals(2u32)]).matches(&v));
    }

    #[test]
    fn option_and_result() {
        let some = Value::Option {
            inner_type: ValueType::U32,
            value: core::option::Option::Some(Box::new(Value::U32(4))),
        };
        let none = Value::Option {
            inner_type: ValueType::U32,
            value: core::option::Option::None,
        };
        assert!(Pattern::some(Pattern::equals(4u32)).matches(&some));
        assert!(!Pattern::some(Pattern::equals(5u32)).matches(&some));
        assert!(Pattern::none().matches(&none));
        assert!(!Pattern::none().matches(&some));

        let ok = Value::Result {
            ok_type: ValueType::U32,
            err_type: ValueType::String,
            value: core::result::Result::Ok(Box::new(Value::U32(1))),
        };
        assert!(Pattern::ok(Pattern::any()).matches(&ok));
        assert!(!Pattern::err(Pattern::any()).matches(&ok));
    }

    #[test]
    fn variant_matches_case_and_payload() {
        let v = Value::Variant {
            type_name: "expr".to_string(),
            case_name: "add".to_string(),
            tag: 1,
            payload: alloc::vec![Value::U32(2), Value::U32(3)],
        };
        assert!(Pattern::variant("add", [Pattern::any(), Pattern::equals(3u32)]).matches(&v));
        assert!(!Pattern::variant("sub", [Pattern::any(), Pattern::any()]).matches(&v));
        // payload arity is exact.
        assert!(!Pattern::variant("add", [Pattern::any()]).matches(&v));
    }

    #[test]
    fn nested_filter_shape() {
        // A realistic subscription filter: an event record whose `header.kind`
        // is "message", ignoring everything else.
        let event = rec(&[
            (
                "header",
                rec(&[("kind", s("message")), ("from", s("alice"))]),
            ),
            ("body", s("hello")),
        ]);
        let filter = Pattern::record_open([(
            "header",
            Pattern::record_open([("kind", Pattern::equals("message".to_string()))]),
        )]);
        assert!(filter.matches(&event));

        let other = rec(&[("header", rec(&[("kind", s("tick"))]))]);
        assert!(!filter.matches(&other));
    }

    fn roundtrip(p: Pattern) {
        let v: Value = p.clone().into();
        let back = Pattern::try_from(v).expect("decode pattern");
        assert_eq!(p, back);
        // And it survives the graph wire codec.
        let bytes = crate::encode(&p.clone().into()).expect("encode");
        let decoded = crate::decode(&bytes).expect("decode bytes");
        let back2 = Pattern::try_from(decoded).expect("pattern from decoded");
        assert_eq!(p, back2);
    }

    #[test]
    fn wire_roundtrip_all_arms() {
        roundtrip(Pattern::any());
        roundtrip(Pattern::equals(42u32));
        roundtrip(Pattern::none());
        roundtrip(Pattern::some(Pattern::equals(1u32)));
        roundtrip(Pattern::ok(Pattern::any()));
        roundtrip(Pattern::err(Pattern::equals("bad".to_string())));
        roundtrip(Pattern::tuple([Pattern::any(), Pattern::equals(2u32)]));
        roundtrip(Pattern::list_open([Pattern::equals(1u32)]));
        roundtrip(Pattern::set([Pattern::any(), Pattern::equals(3u32)]));
        roundtrip(Pattern::variant("add", [Pattern::any()]));
        roundtrip(Pattern::record_open([(
            "kind",
            Pattern::equals("m".to_string()),
        )]));
        roundtrip(Pattern::map([(s("k"), Pattern::any())]));
        // deep nesting
        roundtrip(Pattern::record([(
            "x",
            Pattern::list([Pattern::some(Pattern::map_open([(
                Value::U32(1),
                Pattern::any(),
            )]))]),
        )]));
    }
}
