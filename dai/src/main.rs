fn main() {
    if let Err(e) = dai::run() {
        eprintln!("error: {e}");
    }
}
