use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../THIRD_PARTY_LICENSES.md");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let generated_licenses = manifest_dir.join("../../THIRD_PARTY_LICENSES.md");
    let staged_licenses = out_dir.join("THIRD_PARTY_LICENSES.md");
    if generated_licenses.is_file() {
        fs::copy(&generated_licenses, &staged_licenses)
            .expect("could not stage generated THIRD_PARTY_LICENSES.md");
    } else {
        fs::write(
            &staged_licenses,
            "# Rust dependency licenses\n\n\
This generated bundle is not present in a source checkout. Release packaging \
regenerates THIRD_PARTY_LICENSES.md from Cargo.lock with cargo-about 0.9.2 before \
compiling distributable artifacts.\n",
        )
        .expect("could not write dependency-license development fallback");
    }

    println!("cargo:rerun-if-changed=../../packaging/icons/calibraw.ico");
    println!("cargo:rerun-if-changed=../../packaging/windows/calibraw.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        panic!("CalibRaw's Windows icon embedding currently supports the packaged GNU target");
    }

    let resource_dir = manifest_dir.join("../../packaging/windows");
    let output = out_dir.join("calibraw-icon.o");
    let status = Command::new("windres")
        .current_dir(&resource_dir)
        .args([
            "--input",
            "calibraw.rc",
            "--output-format",
            "coff",
            "--output",
        ])
        .arg(&output)
        .status()
        .unwrap_or_else(|error| panic!("could not start windres: {error}"));
    assert!(
        status.success(),
        "windres failed while embedding the CalibRaw icon"
    );
    println!("cargo:rustc-link-arg-bin=calibraw={}", output.display());
}
