fn main() {
    if let Err(error) = zerdr::run() {
        eprintln!("zerdr: {error}");
        std::process::exit(error.exit_code());
    }
}
