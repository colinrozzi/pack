// Cross-file import: pull `entry` from the shared file. `entry` transitively
// references `msg` and `kind`, so the resolver must auto-pull those too even
// though only `entry` is named here.
use "../use-shared.pact".{entry};

record snapshot {
    latest: entry,
    count: u64,
}

world use-consumer {
    export snap: func(s: snapshot) -> snapshot
}
