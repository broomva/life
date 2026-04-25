//! Smoke tests for the `soma` binary's CLI subcommands.
//!
//! These tests verify that the binary compiles, the clap argument tree is
//! wired correctly, and every top-level subcommand exists — without requiring
//! a live soma daemon (that's BRO-903's job).

use std::process::Command;

/// Returns the path to the compiled `soma` binary provided by Cargo.
///
/// `CARGO_BIN_EXE_soma` is injected by Cargo when running integration tests
/// from the crate that declares the binary, so this always points to the
/// freshly compiled artifact.
fn soma() -> Command {
    Command::new(env!("CARGO_BIN_EXE_soma"))
}

#[test]
fn help_exits_zero() {
    let output = soma()
        .arg("--help")
        .output()
        .expect("failed to run soma --help");

    assert!(
        output.status.success(),
        "soma --help exited with {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn help_mentions_subcommands() {
    let output = soma()
        .arg("--help")
        .output()
        .expect("failed to run soma --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("daemon"),
        "expected 'daemon' in --help output:\n{stdout}"
    );
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
    let output = soma()
        .args(["create-vm", "--help"])
        .output()
        .expect("failed to run soma create-vm --help");

    assert!(
        output.status.success(),
        "soma create-vm --help exited with {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--backend"),
        "expected '--backend' in create-vm --help output:\n{stdout}"
    );
    assert!(
        stdout.contains("--socket"),
        "expected '--socket' in create-vm --help output:\n{stdout}"
    );
}

#[test]
fn dispatch_help_mentions_vm_id_and_tool_name() {
    let output = soma()
        .args(["dispatch", "--help"])
        .output()
        .expect("failed to run soma dispatch --help");

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
    let output = soma()
        .args(["list-vms", "--help"])
        .output()
        .expect("failed to run soma list-vms --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--session"),
        "expected '--session':\n{stdout}"
    );
}
