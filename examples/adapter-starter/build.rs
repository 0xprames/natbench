fn main() {
    use sha2::{Digest, Sha256};
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(compiler)
        .arg("--version")
        .output()
        .expect("rustc version");
    assert!(output.status.success());
    println!(
        "cargo:rustc-env=ADAPTER_RUSTC={}",
        String::from_utf8(output.stdout).unwrap().trim()
    );
    println!(
        "cargo:rustc-env=ADAPTER_PROFILE={}",
        std::env::var("PROFILE").unwrap()
    );
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut hash = Sha256::new();
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src/main.rs",
        "src/transport.rs",
    ] {
        let bytes = std::fs::read(root.join(path)).expect("adapter source");
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
        println!("cargo:rerun-if-changed={path}");
    }
    println!(
        "cargo:rustc-env=ADAPTER_SOURCE_SHA256={:x}",
        hash.finalize()
    );
}
