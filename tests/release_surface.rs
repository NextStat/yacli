use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn homebrew_formula_includes_linux_arm64_release_asset() {
    let temp = tempdir().expect("tempdir");
    let output = temp.path().join("yacli.rb");

    let status = Command::new("sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "./scripts/generate-homebrew-formula.sh",
            "--version",
            "0.2.0",
            "--base-url",
            "https://example.com/releases",
            "--output",
            output.to_str().expect("utf8 path"),
            "--sha256-linux-arm64",
            "arm64sha",
            "--sha256-linux-x64",
            "x64sha",
            "--sha256-macos-arm64",
            "macarm64sha",
        ])
        .status()
        .expect("formula script runs");

    assert!(status.success());

    let formula = fs::read_to_string(&output).expect("formula");
    assert!(formula.contains("yacli-aarch64-unknown-linux-gnu.tar.gz"));
    assert!(formula.contains("sha256 \"arm64sha\""));
    assert!(formula.contains("elsif Hardware::CPU.intel?"));
    assert!(formula.contains("sha256 \"x64sha\""));
}
