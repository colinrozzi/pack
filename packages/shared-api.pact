// Shared Pact definition consumed by the `pact-from-file` fixture via
// `pact!(from "../shared-api.pact")`. The point of this file is that it lives
// OUTSIDE the consuming crate — several crates could point at this one file
// instead of each vendoring/symlinking a copy.

record greeting {
    to: string,
    times: u32,
}

world shared-api {
    export greet: func(g: greeting) -> greeting
}
