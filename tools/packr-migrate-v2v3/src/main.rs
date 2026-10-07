//! One-shot CGRF graph-format v2 -> v3 store-blob migrator.
//!
//! Background: packr graph wire VERSION was 2 for packr 0.11-0.20 and 3 from
//! 0.23 (the map/set-first-class break, #162). MAGIC "CGRF" and every non-map/set
//! payload encoding are byte-identical across v2/v3 — only the header VERSION and
//! the (additive) Map/Set node kinds differ. So decoding v2 bytes with a v2 reader
//! and re-encoding with a v3 writer is a lossless round-trip (for map/set-bearing
//! data it re-encodes the map/set nodes correctly; for map/set-free data it is
//! effectively a header version bump).
//!
//! Reader = packr-abi 0.20.0 (VERSION 2). Writer = packr-abi 0.24.1 (VERSION 3).
//!
//! Usage:
//!   packr-migrate-v2v3 check   <in>            # report CGRF version of <in> (no write)
//!   packr-migrate-v2v3 migrate <in> <out>      # v2 -> v3, verified; refuses non-v2 input
//!
//! Safety: `migrate` REFUSES anything that is not a CGRF v2 blob (plain-byte store
//! entries like dkim-key / api-bearer-token / raw:* have no CGRF header and must be
//! skipped — the tool exits non-zero rather than corrupt them), and it re-decodes
//! its own v3 output with the 0.24.1 reader before writing, so a bad migration
//! fails loudly instead of producing a corrupt blob.
//!
//! ⚠️ CONTENT-ADDRESSED STORE — DO NOT OVERWRITE THE HASH-NAMED FILE IN PLACE.
//! The theater store dedups by content hash: identical content (e.g. every empty
//! MailboxState) is ONE shared `data/<sha1>` file that MULTIPLE labels point at.
//! Overwriting that file with one label's migrated bytes CROSS-CORRUPTS every
//! other mailbox sharing it, and breaks the content-address invariant. So write
//! the migrated blob to a NEW hash-named file and repoint the label
//! (`replace-content-at-label` semantics); NEVER run `migrate X X` onto a store
//! blob, and never `cp` the output over the old `data/<hash>`. Migrate to a
//! scratch path, then store-new + repoint. (In-place `migrate X X` is safe ONLY
//! for a standalone, non-content-addressed file.)

use std::process::ExitCode;

use packr_abi::Value as V3;
use packr_abi::ValueType as T3;
use packr_abi_v2::Value as V2;
use packr_abi_v2::ValueType as T2;

const MAGIC: &[u8; 4] = b"CGRF";

/// Read the CGRF header: returns Some(version) if bytes start with the CGRF magic,
/// else None (not a CGRF blob). Header = magic:u32 le, version:u16 le, ...
fn cgrf_version(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 6 || &bytes[0..4] != MAGIC {
        return None;
    }
    Some(u16::from_le_bytes([bytes[4], bytes[5]]))
}

fn conv_vt(t: T2) -> T3 {
    match t {
        T2::Bool => T3::Bool,
        T2::U8 => T3::U8,
        T2::U16 => T3::U16,
        T2::U32 => T3::U32,
        T2::U64 => T3::U64,
        T2::S8 => T3::S8,
        T2::S16 => T3::S16,
        T2::S32 => T3::S32,
        T2::S64 => T3::S64,
        T2::F32 => T3::F32,
        T2::F64 => T3::F64,
        T2::Char => T3::Char,
        T2::String => T3::String,
        T2::List(inner) => T3::List(Box::new(conv_vt(*inner))),
        T2::Option(inner) => T3::Option(Box::new(conv_vt(*inner))),
        T2::Result { ok, err } => T3::Result {
            ok: Box::new(conv_vt(*ok)),
            err: Box::new(conv_vt(*err)),
        },
        T2::Record(name) => T3::Record(name),
        T2::Variant(name) => T3::Variant(name),
        T2::Tuple(ts) => T3::Tuple(ts.into_iter().map(conv_vt).collect()),
        T2::Flags => T3::Flags,
    }
}

fn conv_value(v: V2) -> V3 {
    match v {
        V2::Bool(b) => V3::Bool(b),
        V2::U8(x) => V3::U8(x),
        V2::U16(x) => V3::U16(x),
        V2::U32(x) => V3::U32(x),
        V2::U64(x) => V3::U64(x),
        V2::S8(x) => V3::S8(x),
        V2::S16(x) => V3::S16(x),
        V2::S32(x) => V3::S32(x),
        V2::S64(x) => V3::S64(x),
        V2::F32(x) => V3::F32(x),
        V2::F64(x) => V3::F64(x),
        V2::Char(c) => V3::Char(c),
        V2::String(s) => V3::String(s),
        V2::List { elem_type, items } => V3::List {
            elem_type: conv_vt(elem_type),
            items: items.into_iter().map(conv_value).collect(),
        },
        V2::Option { inner_type, value } => V3::Option {
            inner_type: conv_vt(inner_type),
            value: value.map(|b| Box::new(conv_value(*b))),
        },
        V2::Result {
            ok_type,
            err_type,
            value,
        } => V3::Result {
            ok_type: conv_vt(ok_type),
            err_type: conv_vt(err_type),
            value: match value {
                Ok(b) => Ok(Box::new(conv_value(*b))),
                Err(b) => Err(Box::new(conv_value(*b))),
            },
        },
        V2::Record { type_name, fields } => V3::Record {
            type_name,
            fields: fields
                .into_iter()
                .map(|(k, val)| (k, conv_value(val)))
                .collect(),
        },
        V2::Variant {
            type_name,
            case_name,
            tag,
            payload,
        } => V3::Variant {
            type_name,
            case_name,
            tag,
            payload: payload.into_iter().map(conv_value).collect(),
        },
        V2::Tuple(items) => V3::Tuple(items.into_iter().map(conv_value).collect()),
        V2::Flags(bits) => V3::Flags(bits),
    }
}

/// Migrate one CGRF v2 blob to v3. Errors (as a message) if the input is not a
/// CGRF v2 blob, if v2 decode fails, or if the produced v3 bytes don't re-decode.
fn migrate_blob(input: &[u8]) -> Result<Vec<u8>, String> {
    match cgrf_version(input) {
        None => Err("input is not a CGRF blob (no magic) — skip, do NOT migrate".into()),
        Some(3) => Err("input is already CGRF v3 — nothing to do".into()),
        Some(2) => {
            let v2 = packr_abi_v2::decode(input).map_err(|e| format!("v2 decode failed: {e:?}"))?;
            let v3 = conv_value(v2);
            let out = packr_abi::encode(&v3).map_err(|e| format!("v3 encode failed: {e:?}"))?;
            // Verify: the produced bytes must decode cleanly under the 0.24.1 reader.
            packr_abi::decode(&out)
                .map_err(|e| format!("verify failed: v3 output did not re-decode: {e:?}"))?;
            Ok(out)
        }
        Some(other) => Err(format!("unexpected CGRF version {other} (expected 2)")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("check") => {
            let Some(path) = args.get(2) else {
                eprintln!("usage: packr-migrate-v2v3 check <in>");
                return ExitCode::from(2);
            };
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("read {path}: {e}");
                    return ExitCode::from(2);
                }
            };
            match cgrf_version(&bytes) {
                Some(v) => println!("{path}: CGRF v{v} ({} bytes)", bytes.len()),
                None => println!("{path}: NOT CGRF ({} bytes) — plain-byte entry, skip", bytes.len()),
            }
            ExitCode::SUCCESS
        }
        Some("migrate") => {
            let (Some(inp), Some(outp)) = (args.get(2), args.get(3)) else {
                eprintln!("usage: packr-migrate-v2v3 migrate <in> <out>");
                return ExitCode::from(2);
            };
            let bytes = match std::fs::read(inp) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("read {inp}: {e}");
                    return ExitCode::from(2);
                }
            };
            match migrate_blob(&bytes) {
                Ok(out) => {
                    if let Err(e) = std::fs::write(outp, &out) {
                        eprintln!("write {outp}: {e}");
                        return ExitCode::from(2);
                    }
                    println!(
                        "migrated {inp} (v2, {} bytes) -> {outp} (v3, {} bytes)",
                        bytes.len(),
                        out.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("SKIP/FAIL {inp}: {e}");
                    // exit 1 = did not migrate (skip or error); caller must not treat as done
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            eprintln!(
                "packr-migrate-v2v3 — CGRF graph v2->v3 store-blob migrator\n\
                 usage:\n  packr-migrate-v2v3 check   <in>\n  packr-migrate-v2v3 migrate <in> <out>\n\
                 \n\
                 CONTENT-ADDRESSED STORE: migrate to a SCRATCH path, then store-new + repoint\n\
                 the label (replace-content-at-label). NEVER overwrite data/<hash> in place —\n\
                 dedup means it may be shared across mailboxes (cross-corruption)."
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a representative map/set-free Value with the v2 crate, encode it as a
    // real v2 blob, migrate, and assert the v3 reader decodes an equal structure.
    #[test]
    fn round_trips_a_v2_blob() {
        // Mimic a MailboxState-ish record: strings, list, option, nested record, result.
        let v2 = V2::Record {
            type_name: "mailbox-state".into(),
            fields: vec![
                ("addr".into(), V2::String("inbox-dev@colinrozzi.com".into())),
                (
                    "messages".into(),
                    V2::List {
                        elem_type: T2::Record("message".into()),
                        items: vec![
                            V2::Record {
                                type_name: "message".into(),
                                fields: vec![
                                    ("id".into(), V2::U64(42)),
                                    ("body".into(), V2::String("hello".into())),
                                    (
                                        "reply_to".into(),
                                        V2::Option {
                                            inner_type: T2::String,
                                            value: Some(Box::new(V2::String("x@y".into()))),
                                        },
                                    ),
                                ],
                            },
                        ],
                    },
                ),
                (
                    "last".into(),
                    V2::Result {
                        ok_type: T2::U32,
                        err_type: T2::String,
                        value: Ok(Box::new(V2::U32(7))),
                    },
                ),
                ("tuple".into(), V2::Tuple(vec![V2::Bool(true), V2::S64(-9)])),
                ("flags".into(), V2::Flags(0b1010)),
            ],
        };

        let v2_bytes = packr_abi_v2::encode(&v2).expect("v2 encode");
        assert_eq!(cgrf_version(&v2_bytes), Some(2), "test fixture must be v2");

        let v3_bytes = migrate_blob(&v2_bytes).expect("migrate");
        assert_eq!(cgrf_version(&v3_bytes), Some(3), "output must be v3");

        // The v3 reader must decode it, and the structure must match a directly
        // v3-encoded equivalent (proves the payload survived the round-trip).
        let decoded_v3 = packr_abi::decode(&v3_bytes).expect("v3 decode");
        let expected_v3 = conv_value(v2);
        let reencoded = packr_abi::encode(&expected_v3).expect("v3 re-encode");
        assert_eq!(
            packr_abi::encode(&decoded_v3).expect("v3 re-encode 2"),
            reencoded,
            "migrated v3 blob must encode identically to the converted value"
        );
    }

    #[test]
    fn refuses_non_cgrf() {
        assert!(migrate_blob(b"-----BEGIN DKIM KEY-----").is_err());
    }

    #[test]
    fn refuses_already_v3() {
        let v3 = packr_abi::encode(&V3::U32(1)).unwrap();
        assert_eq!(cgrf_version(&v3), Some(3));
        assert!(migrate_blob(&v3).is_err());
    }
}
