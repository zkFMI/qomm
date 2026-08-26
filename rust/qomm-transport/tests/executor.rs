use qomm_dsl::registry::CircuitRegistry;
use qomm_mpc::program::{build_program, ProgramConfig};
use qomm_transport::executor::{ProgramRegistry, RegisteredProgram};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn fixture(shape: &str, digest: Option<String>) -> (tempfile::TempDir, RegisteredProgram) {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("approved.sh");
    fs::write(&executable, b"#!/bin/sh\nprintf 'ok\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let actual = hex::encode(Sha256::digest(fs::read(&executable).unwrap()));
    let program = RegisteredProgram {
        shape_digest: shape.into(),
        argv: vec![executable.display().to_string()],
        cwd: directory.path().to_path_buf(),
        executable_sha256: digest.unwrap_or(actual),
        timeout_seconds: 5.0,
    };
    (directory, program)
}

#[test]
fn only_a_byte_verified_registered_program_runs() {
    let (_directory, program) = fixture(&"01".repeat(32), None);
    let registry = ProgramRegistry::new(2, vec![program]).unwrap();
    let result = registry
        .execute(&json!({"shape_digest": "01".repeat(32), "slot": 7}))
        .unwrap();
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["slot"], 7);
    assert_eq!(
        result["stdout_digest"],
        hex::encode(Sha256::digest(b"ok\n"))
    );
}

#[test]
fn unknown_shape_and_substituted_binary_are_refused() {
    let (_directory, program) = fixture(&"01".repeat(32), None);
    let registry = ProgramRegistry::new(2, vec![program]).unwrap();
    assert!(registry
        .execute(&json!({"shape_digest": "02".repeat(32), "slot": 7}))
        .unwrap_err()
        .contains("approved"));
    let (_directory, substituted) = fixture(&"01".repeat(32), Some("00".repeat(32)));
    assert!(ProgramRegistry::new(2, vec![substituted])
        .unwrap_err()
        .contains("digest"));
}

#[test]
fn request_arguments_cannot_replace_the_registered_command() {
    let (_directory, program) = fixture(&"01".repeat(32), None);
    let registry = ProgramRegistry::new(2, vec![program]).unwrap();
    let result = registry
        .execute(&json!({
            "shape_digest": "01".repeat(32), "slot": 9,
            "argv": ["/bin/sh", "-c", "false"]
        }))
        .unwrap();
    assert_eq!(result["exit_code"], 0);
}

#[test]
fn approved_source_does_not_authorize_an_unrelated_executable() {
    const RULE: &str = "param mid[99000,101000] half[1,200] slope[0,16]\ninput qty[1,1000]\nask = mid + half + slope * qty\n";
    let directory = tempfile::tempdir().unwrap();
    let config = ProgramConfig::default();
    let source = build_program(&config).unwrap();
    let shape = [
        config.n_mm as u64,
        config.n_parties as u64,
        u64::from(config.bit_length),
    ];
    let mut circuits = CircuitRegistry::default();
    circuits.approve("quote", RULE, &source, &shape).unwrap();
    let executable = fs::canonicalize("/bin/echo").unwrap();
    let program = RegisteredProgram {
        shape_digest: qomm_transport::executor::circuit_shape_digest(&shape),
        argv: vec![
            executable.display().to_string(),
            "{node}".into(),
            "{slot}".into(),
            "{batch_digest}".into(),
        ],
        cwd: directory.path().to_path_buf(),
        executable_sha256: hex::encode(Sha256::digest(fs::read(&executable).unwrap())),
        timeout_seconds: 5.0,
    };

    let error = ProgramRegistry::from_approved_mpc(0, program, &circuits, &config, &shape)
        .expect_err("/bin/echo is not derived from the approved MPC source");
    assert!(
        error.contains("derived from the approved source"),
        "{error}"
    );
}
