fn main() {
    if let Err(e) = dai::main() {
        eprintln!("error: {e}");
    }
}
