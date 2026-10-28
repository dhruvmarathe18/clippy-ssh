// Cargo intentionally omits target rustflags for host build scripts during a cross build.
// The isolated GNU/LLVM build environment still needs Rust's bundled CRT for those tools.
use std::{env, ffi::OsString, process::{Command, exit}};
fn main() {
    let mut args = env::args_os().skip(1);
    let rustc = args.next().expect("rustc executable");
    let mut args: Vec<OsString> = args.collect();
    let target = args.windows(2).find(|p| p[0] == "--target").map(|p| p[1].clone());
    if target.as_ref().is_none_or(|t| t == "x86_64-pc-windows-gnu") {
        args.extend(["-C", "link-self-contained=yes", "-C", "linker-flavor=ld"].map(OsString::from));
    }
    exit(Command::new(rustc).args(args).status().expect("run rustc").code().unwrap_or(1));
}
