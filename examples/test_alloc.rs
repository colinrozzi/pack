use pack::runtime::Runtime;
use pack::abi::Value;

fn main() -> anyhow::Result<()> {
    // Test with direct wasmtime to confirm it works
    println!("=== Testing with direct wasmtime ===");
    test_direct()?;

    // Test with pack Runtime
    println!("\n=== Testing with pack Runtime ===");
    test_pack_runtime()?;

    Ok(())
}

fn test_direct() -> anyhow::Result<()> {
    use wasmtime::*;
    use pack::abi::{encode, decode, Value};
    use pack::runtime::{RESULT_PTR_OFFSET, RESULT_LEN_OFFSET};

    let wasm_path = "/home/colin/work/pack/packages/echo/target/wasm32-unknown-unknown/release/echo_package.wasm";
    let wasm_bytes = std::fs::read(wasm_path)?;

    let engine = Engine::default();
    let module = Module::new(&engine, &wasm_bytes)?;

    let mut store = Store::new(&engine, ());
    let linker = Linker::<()>::new(&engine);
    let instance = linker.instantiate(&mut store, &module)?;
    let memory = instance.get_memory(&mut store, "memory").unwrap();

    let input = Value::S64(42);
    let input_bytes = encode(&input)?;

    let alloc = instance.get_typed_func::<i32, i32>(&mut store, "__pack_alloc")?;
    let in_ptr = alloc.call(&mut store, input_bytes.len() as i32)?;
    println!("Allocated input at: {}", in_ptr);

    memory.write(&mut store, in_ptr as usize, &input_bytes)?;

    let echo = instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "echo")?;
    let status = echo.call(&mut store, (
        in_ptr,
        input_bytes.len() as i32,
        RESULT_PTR_OFFSET as i32,
        RESULT_LEN_OFFSET as i32
    ))?;
    println!("Status: {}", status);

    // Try freeing the input buffer
    println!("Freeing input buffer...");
    let free = instance.get_typed_func::<(i32, i32), ()>(&mut store, "__pack_free")?;
    free.call(&mut store, (in_ptr, input_bytes.len() as i32))?;
    println!("Input buffer freed");

    let mut ptr_bytes = [0u8; 4];
    let mut len_bytes = [0u8; 4];
    memory.read(&store, RESULT_PTR_OFFSET, &mut ptr_bytes)?;
    memory.read(&store, RESULT_LEN_OFFSET, &mut len_bytes)?;

    let out_ptr = i32::from_le_bytes(ptr_bytes) as usize;
    let out_len = i32::from_le_bytes(len_bytes) as usize;

    let mut output_bytes = vec![0u8; out_len];
    memory.read(&store, out_ptr, &mut output_bytes)?;
    let output = decode(&output_bytes)?;
    println!("Output: {:?}", output);

    Ok(())
}

fn test_pack_runtime() -> anyhow::Result<()> {
    let wasm_path = "/home/colin/work/pack/packages/echo/target/wasm32-unknown-unknown/release/echo_package.wasm";
    let wasm_bytes = std::fs::read(wasm_path)?;

    let runtime = Runtime::new();
    let module = runtime.load_module(&wasm_bytes)?;
    let mut instance = module.instantiate()?;

    let input = Value::S64(42);
    println!("Calling echo...");
    let output = instance.call_with_value("echo", &input)?;
    println!("Output: {:?}", output);

    Ok(())
}
