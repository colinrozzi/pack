//! Convergence test: handler-side and actor-side hashing of a record-using
//! interface must produce byte-identical interface hashes.
//!
//! The handler side parses .pact source via `parse_pact`, builds an
//! `InterfaceImpl::from_pact`, and reports its hash — this is what theater
//! does at handler-registration time. The actor side (proc-macro) computes
//! its hash through `packr_abi::hash_*`. This test verifies that both
//! pipelines produce the same bytes when given the same interface definition,
//! which is the property theater checks before wiring an actor to a handler.
//!
//! Together these cover the agentry-actor + theater-handler-podman bug:
//! the actor declares records inside `imports { theater:simple/podman { ... } }`
//! and the resulting hash matches the handler-side hash of the same interface
//! defined in podman.pact.

use packr::{parse_pact, InterfaceImpl};

#[test]
fn handler_hash_matches_actor_hash_for_record_interface() {
    // The exact shape from theater-handler-podman/podman.pact, trimmed to two
    // functions for clarity. Records live inside the interface block.
    let src = r#"
        interface podman {
            @package: string = "theater:simple"

            record mount-spec {
                source: string,
                target: string,
                read-only: bool,
            }

            record container-spec {
                image: string,
                name: string,
                mounts: list<mount-spec>,
            }

            exports {
                run: func(spec: container-spec) -> result<string, string>
                stop: func(name: string) -> result<_, string>
            }
        }
    "#;

    // Handler side: the path theater actually takes — parse pact, build
    // InterfaceImpl, ask for its hash.
    let pact = parse_pact(src).expect("parse pact");
    let iface = InterfaceImpl::from_pact(&pact);
    let handler_hash = iface.hash();

    // Actor side: compute the hash directly from the structural primitives
    // exposed by packr-abi, which is exactly what pack_types! does at
    // proc-macro time (`packr_guest_macros::metadata::compute_interface_hashes`).
    use packr_abi::{
        hash_function, hash_interface, hash_list, hash_record, hash_result, Binding, HASH_BOOL,
        HASH_STRING,
    };

    let mount_spec_hash = hash_record(&[
        ("read-only", HASH_BOOL),
        ("source", HASH_STRING),
        ("target", HASH_STRING),
    ]);
    let container_spec_hash = hash_record(&[
        ("image", HASH_STRING),
        ("mounts", hash_list(&mount_spec_hash)),
        ("name", HASH_STRING),
    ]);

    let run_hash = hash_function(
        &[container_spec_hash],
        &[hash_result(&HASH_STRING, &HASH_STRING)],
    );
    // `result<_, string>` — by convention, `_` in ok position hashes as Bool
    // and in err position as String (see pact.rs::parse_result).
    let stop_hash = hash_function(&[HASH_STRING], &[hash_result(&HASH_BOOL, &HASH_STRING)]);

    let mut bindings = vec![
        Binding {
            name: "run",
            hash: run_hash,
        },
        Binding {
            name: "stop",
            hash: stop_hash,
        },
    ];
    bindings.sort_by(|a, b| a.name.cmp(b.name));

    let actor_hash = hash_interface("theater:simple/podman", &[], &bindings);

    // Convergence: byte-equal across the two hashers.
    assert_eq!(
        handler_hash.as_bytes(),
        actor_hash.as_bytes(),
        "handler hash and actor hash must agree for record-using interfaces"
    );
}
