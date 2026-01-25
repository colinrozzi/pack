// Generate bindings for the doubler world
wit_bindgen::generate!({
    world: "doubler",
    path: "../wit/doubler.wit",
});

struct Doubler;

impl Guest for Doubler {
    fn double(n: i64) -> i64 {
        n * 2
    }
}

export!(Doubler);
