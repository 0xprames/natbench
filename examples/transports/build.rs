fn main() {
    use sha2::{Digest, Sha256};
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(compiler)
        .arg("--version")
        .output()
        .expect("rustc version");
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout).unwrap();
    println!("cargo:rustc-env=ADAPTER_RUSTC={}", version.trim());
    println!(
        "cargo:rustc-env=ADAPTER_PROFILE={}",
        std::env::var("PROFILE").unwrap()
    );
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut hash = Sha256::new();
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src/main.rs",
        "src/connectivity.rs",
        "../../crates/transport-protocol/Cargo.toml",
        "../../crates/transport-protocol/src/lib.rs",
    ] {
        let bytes = std::fs::read(manifest.join(path)).expect("adapter source");
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        println!("cargo:rerun-if-changed={path}");
    }
    println!(
        "cargo:rustc-env=ADAPTER_SOURCE_SHA256={:x}",
        hash.finalize()
    );
    let lock = std::fs::read_to_string(manifest.join("Cargo.lock")).expect("adapter lockfile");
    let mut name = "";
    for line in lock.lines() {
        if let Some(value) = line.strip_prefix("name = \"") {
            name = value.trim_end_matches('"');
        }
        if name == "noq" {
            if let Some(value) = line.strip_prefix("version = \"") {
                println!(
                    "cargo:rustc-env=ADAPTER_NOQ={}",
                    value.trim_end_matches('"')
                );
            }
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
}
