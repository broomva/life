//! Smoke tests for the `lifectl` binary.
//!
//! These tests verify that the binary compiles, the clap argument tree is
//! wired correctly, and every top-level subcommand exists — without requiring
//! a live lifed daemon (that's BRO-903's job).

use std::process::Command;

/// Returns the path to the compiled `lifectl` binary provided by Cargo.
///
/// `CARGO_BIN_EXE_lifectl` is injected by Cargo when running integration
/// tests from the crate that declares the binary, so this always points to
/// the freshly compiled artifact.
fn lifectl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lifectl"))
}

#[test]
fn help_exits_zero() {
    let output = lifectl()
        .arg("--help")
        .output()
        .expect("failed to run lifectl --help");

    assert!(
        output.status.success(),
        "lifectl --help exited with {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn help_mentions_subcommands() {
    let output = lifectl()
        .arg("--help")
        .output()
        .expect("failed to run lifectl --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("create-vm"),
        "expected 'create-vm' in --help output:\n{stdout}"
    );
    assert!(
        stdout.contains("dispatch"),
        "expected 'dispatch' in --help output:\n{stdout}"
    );
    assert!(
        stdout.contains("list-vms"),
        "expected 'list-vms' in --help output:\n{stdout}"
    );
}

#[test]
fn create_vm_help_exits_zero_and_mentions_backend() {
    let output = lifectl()
        .args(["create-vm", "--help"])
        .output()
        .expect("failed to run lifectl create-vm --help");

    assert!(
        output.status.success(),
        "lifectl create-vm --help exited with {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--backend"),
        "expected '--backend' in create-vm --help output:\n{stdout}"
    );
}

#[test]
fn dispatch_help_mentions_vm_id_and_tool_name() {
    let output = lifectl()
        .args(["dispatch", "--help"])
        .output()
        .expect("failed to run lifectl dispatch --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--vm-id"), "expected '--vm-id':\n{stdout}");
    assert!(
        stdout.contains("--tool-name"),
        "expected '--tool-name':\n{stdout}"
    );
}

#[test]
fn list_vms_help_mentions_session() {
    let output = lifectl()
        .args(["list-vms", "--help"])
        .output()
        .expect("failed to run lifectl list-vms --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--session"),
        "expected '--session':\n{stdout}"
    );
}
