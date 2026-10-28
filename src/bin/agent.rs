fn main() {
    #[cfg(unix)]
    if let Err(e) = clippy_ssh::agent::run() {
        eprintln!("clippy-agent: {e:#}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        eprintln!("clippy-agent runs on Linux; use clippy.exe on Windows.");
        std::process::exit(1);
    }
}
