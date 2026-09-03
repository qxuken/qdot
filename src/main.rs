fn main() {
    if let Err(e) = qd::cli::run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
