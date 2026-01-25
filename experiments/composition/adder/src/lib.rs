// Generate bindings for the adder world (which imports doubler)
wit_bindgen::generate!({
    world: "adder",
    path: "../wit/adder.wit",
    with: {
        "process:doubler/math": generate,
    },
});

use process::doubler::math;

struct Adder;

impl Guest for Adder {
    fn process(n: i64) -> i64 {
        // First double the number using the imported function
        let doubled = math::double(n);
        // Then add 1
        doubled + 1
    }
}

export!(Adder);
