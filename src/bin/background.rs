#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    if clippy_ssh::windows::background::run().is_err() {
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!("The background app runs on Windows.");
}
