  Composite Requirements for Theater Integration

⏺ Based on what I'm seeing in both codebases, here's the refined list:

  1. Engine Abstraction (wasmtime support)

  Composite currently has wasmi hardcoded at line 16 of runtime/mod.rs. We need:

  // Something like this trait
  pub trait WasmEngine: Send + Sync {
      type Module;
      type Instance;
      type Store<T>;
      type Linker<T>;

      fn compile(&self, bytes: &[u8]) -> Result<Self::Module>;
      fn instantiate<T>(&self, module: &Self::Module, linker: &Self::Linker<T>, store: &mut Self::Store<T>) -> Result<Self::Instance>;
      // ... memory access, function calls, etc.
  }

  Then implement for both wasmi and wasmtime.

  ---
  2. Async Support

  Theater uses func_wrap_async extensively for I/O operations. Two options:
  - Full async: Make the engine trait async-native
  - Hybrid: Keep sync core, add call_async that wraps in spawn_blocking

  The engine abstraction needs to account for this - wasmtime has async support, wasmi doesn't.

  ---
  3. Host Function Registration API

  This is the big one. Theater's pattern:

  // Theater does this ~167 times across handlers
  let mut interface = linker.instance("theater:simple/runtime")?;
  interface.func_wrap("log", |ctx: StoreContextMut<ActorStore>, (msg,): (String,)| {
      // access ctx.data() for state
      Ok(())
  })?;

  Composite currently has:
  // Hard-coded in register_host_functions()
  linker.func_wrap("host", "log", |caller, ptr, len| { ... });

  What we need:
  - Namespaced interfaces (theater:simple/runtime, not just host)
  - Dynamic registration (not hard-coded)
  - Access to typed store context from host functions
  - Both sync and async variants
  - Maybe a builder pattern:

  let mut linker = CompositeLinker::new(&engine);
  linker
      .interface("theater:simple/runtime")?
      .func("log", |ctx: &mut StoreContext<MyState>, msg: String| {
          ctx.data().log(msg);
          Ok(())
      })?
      .func_async("load", |ctx, path: String| async move {
          // async operation
          Ok(bytes)
      })?;

  ---
  4. WIT+ Development

  The parser exists but needs:

  Imports/Exports structure:
  // Currently Composite parses interfaces but doesn't have world/import/export
  world my-actor {
      import theater:simple/runtime;
      import theater:simple/store;

      export actor {
          init: func(state: option<list<u8>>) -> result<_, string>;
          handle: func(msg: message) -> result<response, string>;
      }
  }

  Validation against component:
  - Check that component exports match expected interface
  - Check that component imports are satisfied by host

  ---
  Summary Table

  | Feature        | Current State                   | Needed                               |
  |----------------|---------------------------------|--------------------------------------|
  | Engine         | wasmi only                      | Trait abstraction + wasmtime impl    |
  | Async          | Sync only                       | At least async function calls        |
  | Host Functions | Hard-coded host.log, host.alloc | Dynamic registration with namespaces |
  | Store Context  | HostState with log/alloc        | Generic T with typed access          |
  | WIT+ Imports   | Not implemented                 | Parse & validate import declarations |
  | WIT+ Exports   | Basic validation                | Full world/export support            |

  ---
  Does this match your understanding? I'd prioritize them as:

  1. Host function API - This is the biggest blocker; without it, you can't port handlers
  2. Engine abstraction - Needed for performance (wasmtime)
  3. Async support - Required for I/O handlers
  4. WIT+ imports/exports - Nice to have for validation, but could start without full support

  Want me to dive deeper into any of these, or should we start sketching out the API design for one of them?
