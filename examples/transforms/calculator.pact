// Base calculator interface - what actors implement
interface calculator {
    exports {
        add: func(a: s32, b: s32) -> s32
        divide: func(a: s32, b: s32) -> result<s32, string>
        reset: func()
    }
}
