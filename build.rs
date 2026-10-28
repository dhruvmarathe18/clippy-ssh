use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=dist/agents");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for arch in ["x86_64", "aarch64"] {
        let source = PathBuf::from(format!("dist/agents/clippy-agent-linux-{arch}"));
        let bytes = fs::read(source).unwrap_or_default();
        fs::write(out.join(format!("agent-{arch}")), bytes).unwrap();
    }
}
