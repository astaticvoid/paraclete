/// CI integration test: validates the Sequencer plugin binary with clap-validator.
///
/// Tagged `#[ignore]` so it does not run in `cargo test` (it requires a built
/// .clap binary and the clap-validator CLI tool to be installed).
///
/// Run manually with:
///   cargo test --test clap_validator -- --ignored
///
/// Prerequisites:
///   cargo install clap-validator
///   cargo build --release  (builds the .clap binary)
#[test]
#[ignore]
fn clap_validator_sequencer_passes() {
    let binary = std::env::var("PARACLETE_CLAP_BINARY")
        .unwrap_or_else(|_| "target/debug/paraclete_sequencer.clap".to_string());

    let status = std::process::Command::new("clap-validator")
        .arg("validate")
        .arg(&binary)
        .status()
        .expect("clap-validator not found — install with: cargo install clap-validator");

    assert!(status.success(), "clap-validator reported failures for {binary}");
}
