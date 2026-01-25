//! Package composition tests
//!
//! These tests verify that packages can be composed together,
//! with one package's imports satisfied by another package's exports.

use composite::abi::Value;
use composite::runtime::CompositionBuilder;
use std::path::Path;

/// Load the doubler package (exports `double`)
fn load_doubler_package() -> Vec<u8> {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packages/doubler/target/wasm32-unknown-unknown/release/doubler_package.wasm");
    std::fs::read(&wasm_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read doubler package at {}: {}. Run: cd packages/doubler && cargo build --target wasm32-unknown-unknown --release",
            wasm_path.display(),
            e
        )
    })
}

/// Load the adder package (imports `double` from "math", exports `process`)
fn load_adder_package() -> Vec<u8> {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packages/adder/target/wasm32-unknown-unknown/release/adder_package.wasm");
    std::fs::read(&wasm_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read adder package at {}: {}. Run: cd packages/adder && cargo build --target wasm32-unknown-unknown --release",
            wasm_path.display(),
            e
        )
    })
}

#[test]
fn composition_doubler_alone() {
    // Test that doubler works standalone
    let doubler_wasm = load_doubler_package();

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .build()
        .expect("failed to build composition");

    // Call double(5) => 10
    let result = composition
        .call("doubler", "double", &Value::S64(5))
        .expect("failed to call double");

    assert_eq!(result, Value::S64(10));
}

#[test]
fn composition_adder_with_doubler() {
    // The main test: compose adder with doubler
    let doubler_wasm = load_doubler_package();
    let adder_wasm = load_adder_package();

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        // Wire adder's import of "math::double" to doubler's "double" export
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("failed to build composition");

    // Call process(5):
    // - adder imports double(5) from "math" module
    // - doubler's double(5) returns 10
    // - adder adds 1: 10 + 1 = 11
    let result = composition
        .call("adder", "process", &Value::S64(5))
        .expect("failed to call process");

    assert_eq!(result, Value::S64(11));
}

#[test]
fn composition_multiple_calls() {
    let doubler_wasm = load_doubler_package();
    let adder_wasm = load_adder_package();

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("failed to build composition");

    // Test with various inputs
    let test_cases = vec![
        (0, 1),   // 0 * 2 + 1 = 1
        (1, 3),   // 1 * 2 + 1 = 3
        (5, 11),  // 5 * 2 + 1 = 11
        (10, 21), // 10 * 2 + 1 = 21
        (100, 201), // 100 * 2 + 1 = 201
    ];

    for (input, expected) in test_cases {
        let result = composition
            .call("adder", "process", &Value::S64(input))
            .expect(&format!("failed to call process with {}", input));
        assert_eq!(result, Value::S64(expected), "process({}) should be {}", input, expected);
    }
}

#[test]
fn composition_list_packages() {
    let doubler_wasm = load_doubler_package();
    let adder_wasm = load_adder_package();

    let composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("failed to build composition");

    let packages = composition.packages();
    assert_eq!(packages.len(), 2);
    assert!(packages.contains(&"doubler".to_string()));
    assert!(packages.contains(&"adder".to_string()));
}

#[test]
fn composition_call_both_packages() {
    let doubler_wasm = load_doubler_package();
    let adder_wasm = load_adder_package();

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("failed to build composition");

    // Can call either package
    let double_result = composition
        .call("doubler", "double", &Value::S64(7))
        .expect("failed to call doubler");
    assert_eq!(double_result, Value::S64(14));

    let process_result = composition
        .call("adder", "process", &Value::S64(7))
        .expect("failed to call adder");
    assert_eq!(process_result, Value::S64(15)); // 7 * 2 + 1
}

#[test]
fn composition_negative_numbers() {
    let doubler_wasm = load_doubler_package();
    let adder_wasm = load_adder_package();

    let mut composition = CompositionBuilder::new()
        .add_package("doubler", doubler_wasm)
        .add_package("adder", adder_wasm)
        .wire("adder", "math", "double", "doubler", "double")
        .build()
        .expect("failed to build composition");

    // Test with negative numbers
    let result = composition
        .call("adder", "process", &Value::S64(-5))
        .expect("failed to call process with negative");

    // -5 * 2 + 1 = -9
    assert_eq!(result, Value::S64(-9));
}
