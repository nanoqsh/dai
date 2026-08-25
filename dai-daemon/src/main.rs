fn main() {
    if let Err(e) = dai_daemon::main() {
        eprintln!("error: {e}");
    }
}
