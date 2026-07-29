use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run(input: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tx-processor"))
        .arg(input)
        .output()
        .expect("failed to run the tx-processor binary");
    assert!(
        output.status.success(),
        "binary exited with a failure status; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout was not valid UTF-8")
}

/// Runs the binary against `{name}_input.csv` and compares its output
/// byte-for-byte against `{name}_expected.csv`. This only works because
/// `Ledger` iterates accounts in a deterministic (client ID) order — with
/// a `HashMap` the same run could emit rows in a different order every
/// time, making an exact-text comparison flaky.
fn assert_golden(name: &str) {
    let input = fixture(&format!("{name}_input.csv"));
    let expected = std::fs::read_to_string(fixture(&format!("{name}_expected.csv")))
        .expect("failed to read expected fixture");

    let actual = run(&input);

    assert_eq!(actual, expected, "golden mismatch for fixture {name:?}");
}

#[test]
fn basic_deposits_and_withdrawals() {
    // The spec's own worked example: a withdrawal exceeding available
    // funds is silently rejected rather than applied.
    assert_golden("basic");
}

#[test]
fn dispute_then_resolve_releases_held_funds() {
    assert_golden("dispute_resolve");
}

#[test]
fn dispute_then_chargeback_locks_the_account() {
    // Also covers that a locked account rejects a subsequent withdrawal.
    assert_golden("dispute_chargeback");
}

#[test]
fn malformed_references_are_ignored_not_fatal() {
    // A dispute on a nonexistent tx and an over-limit withdrawal are both
    // dropped; processing continues and other clients are unaffected.
    assert_golden("ignored_errors");
}

#[test]
fn disputing_a_deposit_after_withdrawing_against_it_goes_negative() {
    // The fraud pattern the assignment itself calls out: deposit, withdraw
    // against it, then dispute the original deposit. The withdrawn funds
    // are no longer there, so holding the disputed amount drives
    // `available` negative while `total` stays correct.
    assert_golden("negative_available");
}

#[test]
fn disputing_an_already_disputed_transaction_is_rejected() {
    assert_golden("already_disputed");
}

#[test]
fn disputing_another_clients_transaction_is_rejected() {
    assert_golden("client_mismatch");
}

#[test]
fn reusing_a_tx_id_is_rejected() {
    assert_golden("duplicate_transaction");
}

#[test]
fn missing_argument_fails_without_panicking() {
    let output = Command::new(env!("CARGO_BIN_EXE_tx-processor"))
        .output()
        .expect("failed to run the tx-processor binary");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn nonexistent_file_fails_without_panicking() {
    let output = Command::new(env!("CARGO_BIN_EXE_tx-processor"))
        .arg("this/path/does/not/exist.csv")
        .output()
        .expect("failed to run the tx-processor binary");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}
