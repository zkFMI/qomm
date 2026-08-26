use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    mode: &'static str,
    makers: usize,
    reference: &'static str,
    persist_wires: bool,
    binding_limit: bool,
    input_check: bool,
    audit_gates: bool,
    shamir_inputs: bool,
}

const CASES: [Case; 6] = [
    Case {
        name: "rfq_4_anchored_all_off",
        mode: "rfq",
        makers: 4,
        reference: "anchored",
        persist_wires: false,
        binding_limit: false,
        input_check: false,
        audit_gates: false,
        shamir_inputs: false,
    },
    Case {
        name: "rfq_16_none_all_on_shamir",
        mode: "rfq",
        makers: 16,
        reference: "none",
        persist_wires: true,
        binding_limit: true,
        input_check: true,
        audit_gates: true,
        shamir_inputs: true,
    },
    Case {
        name: "rfm_4_none_input_shamir",
        mode: "rfm",
        makers: 4,
        reference: "none",
        persist_wires: true,
        binding_limit: false,
        input_check: true,
        audit_gates: false,
        shamir_inputs: true,
    },
    Case {
        name: "rfm_16_anchored_binding_audit",
        mode: "rfm",
        makers: 16,
        reference: "anchored",
        persist_wires: false,
        binding_limit: true,
        input_check: false,
        audit_gates: true,
        shamir_inputs: false,
    },
    Case {
        name: "rfs_4_anchored_binding",
        mode: "rfs",
        makers: 4,
        reference: "anchored",
        persist_wires: true,
        binding_limit: true,
        input_check: false,
        audit_gates: false,
        shamir_inputs: false,
    },
    Case {
        name: "rfs_16_none_input_audit_shamir",
        mode: "rfs",
        makers: 16,
        reference: "none",
        persist_wires: false,
        binding_limit: false,
        input_check: true,
        audit_gates: true,
        shamir_inputs: true,
    },
];

// Recovered from the last exported tree that carried `mp_spdz/gen_qomm.py`,
// after the live parity test had established byte equality for this matrix.
const RETIRED_GENERATOR_PROGRAM_CONTRACT: [(&str, usize, &str); 6] = [
    (
        "rfq_4_anchored_all_off",
        9_453,
        "e819ccf03070e4bcb3bdc4bbb3d96d4575ce65bd2cb417f9ee68d320cf2e5723",
    ),
    (
        "rfq_16_none_all_on_shamir",
        18_160,
        "ac9fc3119a150d7e02e034e3976dc7b545b6d925c2a6793ae882dcbcc983c102",
    ),
    (
        "rfm_4_none_input_shamir",
        11_447,
        "b7dd6eecefbf5ebe2641f88e875cfad919b4aa59d086897e7d2eb1a2e17a2dc3",
    ),
    (
        "rfm_16_anchored_binding_audit",
        11_975,
        "f9299d7b3ff14dde5774d0b1d36c4e73a1c42721e83d953de613063ea8628710",
    ),
    (
        "rfs_4_anchored_binding",
        11_813,
        "50036fbcb57362761475773e654100a5e8b29c0811cf4c2d2d0a3e9262533d17",
    ),
    (
        "rfs_16_none_input_audit_shamir",
        12_440,
        "546183460d6931344763dccb2ed05bffe71a9bf852dee75da7739db418888bc5",
    ),
];

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "qomm-program-parity-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("parity test directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn options(case: Case) -> Vec<String> {
    let mut args = vec![
        "--mode".into(),
        case.mode.into(),
        "--n-mm".into(),
        case.makers.to_string(),
        "--bit-length".into(),
        "31".into(),
        "--reference".into(),
        case.reference.into(),
    ];
    for (enabled, option) in [
        (case.persist_wires && case.mode == "rfq", "--persist-wires"),
        (case.binding_limit, "--binding-limit"),
        (case.input_check, "--input-check"),
        (case.audit_gates, "--audit-gates"),
    ] {
        if enabled {
            args.push(option.into());
        }
    }
    if case.shamir_inputs {
        args.extend([
            "--shamir-inputs".into(),
            "--field-bits".into(),
            "253".into(),
        ]);
    }
    args
}

fn assert_success(label: &str, output: std::process::Output) {
    assert!(
        output.status.success(),
        "{label} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn output_options(program: &Path, inputs: &Path, reference: &Path) -> [String; 6] {
    [
        "--out-program".into(),
        program.display().to_string(),
        "--out-input-dir".into(),
        inputs.display().to_string(),
        "--out-reference".into(),
        reference.display().to_string(),
    ]
}

#[test]
fn rust_cli_matches_the_retired_generator_program_contract() {
    let directory = TestDir::new();
    let rust_generator = env!("CARGO_BIN_EXE_qomm-gen");

    for (case, (contract_name, expected_bytes, expected_sha256)) in
        CASES.into_iter().zip(RETIRED_GENERATOR_PROGRAM_CONTRACT)
    {
        assert_eq!(case.name, contract_name);
        let rust_program = directory.0.join(format!("{}.mpc", case.name));

        // Persistence is an RFQ-only contract. Keep this byte-parity matrix on
        // valid configurations; Rust's non-RFQ refusal is asserted separately
        // in all_files_parity.rs.
        let mut rust_args = options(case);
        rust_args.extend(output_options(
            &rust_program,
            &directory.0.join("rust-input"),
            &directory.0.join("rust-reference.json"),
        ));
        let rust = Command::new(rust_generator)
            .args(&rust_args)
            .output()
            .expect("run Rust generator");
        assert_success(&format!("Rust case {}", case.name), rust);

        let bytes = fs::read(&rust_program).unwrap();
        assert_eq!(
            bytes.len(),
            expected_bytes,
            "case {} changed byte length",
            case.name
        );
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            expected_sha256,
            "case {} changed bytes",
            case.name
        );
    }
}
