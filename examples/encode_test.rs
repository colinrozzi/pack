use pack::abi::{encode, Value};

fn main() {
    // Empty tuple
    let empty_tuple = Value::Tuple(vec![]);
    let bytes = encode(&empty_tuple).unwrap();
    println!("Empty tuple: {} bytes", bytes.len());
    print!("Bytes: ");
    for b in &bytes {
        print!("{:02x} ", b);
    }
    println!();
}
