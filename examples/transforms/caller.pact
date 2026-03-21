// Caller interface - uses transformed calculator
interface caller {
    use rpc(calculator)

    // Now has access to:
    // - add: func(a: s32, b: s32) -> result<s32, rpc-error>
    // - divide: func(a: s32, b: s32) -> result<result<s32, string>, rpc-error>
    // - reset: func() -> result<_, rpc-error>
    // - rpc-error variant type
}
