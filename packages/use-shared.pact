// Shared definitions single-sourced across crates. Consumed both directly
// (pact!(from "../use-shared.pact")) and via cross-file `use` from other .pact
// files (see packages/use-consumer/consumer.pact).

record msg {
    room: string,
    body: list<u8>,
}

variant kind {
    text,
    join,
    leave,
}

record entry {
    author: list<u8>,
    msg: msg,
    kind: kind,
}

world shared {}
