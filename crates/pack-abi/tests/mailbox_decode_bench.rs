//! Regression bench for the mailbox `load_state` decode hang (the mail-spine
//! 0.10.2 prod incident): a mailbox actor restoring a large accumulated
//! `MailboxState { address, messages: Vec<Message> }` from the store hung the
//! whole spine.
//!
//! Two findings this bench nails down, SYNTHETICALLY (no real mail):
//!  1. Decode is LINEAR IN TIME in both the old and fixed decoders — a flat
//!     `Vec<Message>` is shallow, so it was never quadratic-in-time. Decode
//!     compute alone cannot explain a minutes-long spin.
//!  2. The old `cache.insert(value.clone())` deep-clones EVERY node's subtree
//!     into the cache — a storm of allocations and ~2-3x peak bytes. On a fast
//!     host allocator that's just a ~2x constant factor; against the guest's
//!     CAPPED dlmalloc heap (internalize disables `memory.grow`) that blowup is
//!     the plausible spin. The fixed decoder caches only shared (multi-parent)
//!     nodes, so a tree clones NOTHING — allocations drop to O(nodes) and peak
//!     bytes to ~1x the value.
//!
//! Run: `cargo test -p packr-abi --test mailbox_decode_bench -- --nocapture --test-threads=1`

use packr_abi::{decode, encode, Value, ValueType};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

/// Counting allocator: tracks live bytes, peak bytes, and allocation count so
/// the bench can measure the decoder's allocation behaviour (the axis that
/// actually differs between old and fixed), not just wall time.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            COUNT.fetch_add(1, Relaxed);
            // LIVE is a never-reset running total (add/sub balanced, stays >= 0);
            // PEAK is reset per window and tracks the high-water mark of LIVE.
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Open a measurement window: reset the alloc COUNT and pin PEAK to the current
/// live baseline. Returns that baseline so peak-above-baseline can be reported.
/// LIVE is never zeroed (that would underflow on later deallocs of pre-window
/// memory), so the window measures the decoder's own footprint above whatever
/// the still-alive input value/bytes already occupy.
fn open_window() -> usize {
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    COUNT.store(0, Relaxed);
    base
}

fn big_body(body_len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    (0..body_len)
        .map(|i| ALPHABET[i % ALPHABET.len()] as char)
        .collect()
}

/// One `Message` record — the exact field set the inbox mailbox actor persists.
fn message(id: u64, body: &str) -> Value {
    Value::Record {
        type_name: "message".into(),
        fields: vec![
            ("id".into(), Value::U64(id)),
            ("from".into(), Value::String("sender@example.com".into())),
            ("to".into(), Value::String("mailbox@example.com".into())),
            (
                "subject".into(),
                Value::String(format!("Subject line {id}")),
            ),
            ("body".into(), Value::String(body.into())),
            ("received_at".into(), Value::U64(1_700_000_000 + id)),
            (
                "message_id".into(),
                Value::String(format!("<{id}@example.com>")),
            ),
            ("in_reply_to".into(), Value::String(String::new())),
            ("references".into(), Value::String(String::new())),
            (
                "thread_id".into(),
                Value::String(format!("<{id}@example.com>")),
            ),
        ],
    }
}

fn mailbox_state(n: u64, body_len: usize) -> Value {
    let body = big_body(body_len);
    let messages: Vec<Value> = (0..n).map(|id| message(id, &body)).collect();
    Value::Record {
        type_name: "mailbox-state".into(),
        fields: vec![
            (
                "address".into(),
                Value::String("mailbox@example.com".into()),
            ),
            (
                "messages".into(),
                Value::List {
                    elem_type: ValueType::Record("message".into()),
                    items: messages,
                },
            ),
        ],
    }
}

#[test]
fn mailbox_decode_sweep_is_linear() {
    const BODY: usize = 4096;
    eprintln!("\n=== mailbox load_state decode sweep (body={BODY}B) ===");
    eprintln!(
        " {:>6} | {:>10} | {:>10} | {:>8} | {:>10} | {:>9}",
        "msgs", "bytes", "decode", "ns/byte", "allocs", "peakKB"
    );

    let mut prev: Option<(u64, u128)> = None;
    for &n in &[50u64, 100, 200, 400, 800, 1600] {
        let value = mailbox_state(n, BODY);
        let bytes = encode(&value).expect("encode");

        let base = open_window();
        let t = Instant::now();
        let decoded = decode(&bytes).expect("decode");
        let elapsed = t.elapsed();
        let allocs = COUNT.load(Relaxed);
        let peak_kb = PEAK.load(Relaxed).saturating_sub(base) / 1024;

        assert_eq!(decoded, value, "round-trip mismatch at n={n}");
        eprintln!(
            " {:>6} | {:>10} | {:>7} µs | {:>8.2} | {:>10} | {:>7} KB",
            n,
            bytes.len(),
            elapsed.as_micros(),
            elapsed.as_nanos() as f64 / bytes.len() as f64,
            allocs,
            peak_kb
        );

        if let Some((pn, pt)) = prev {
            if n == pn * 2 && pt > 50 {
                let ratio = elapsed.as_nanos() as f64 / pt as f64;
                assert!(
                    ratio < 3.0,
                    "decode super-linear: n {pn}->{n} time x{ratio:.1} (>3x for 2x data)"
                );
            }
        }
        prev = Some((n, elapsed.as_nanos()));
    }
}
