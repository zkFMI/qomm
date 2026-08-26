use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
struct Profile {
    name: &'static str,
    makers: usize,
    reference: &'static str,
    persist_wires: bool,
    binding_limit: bool,
    input_check: bool,
    audit_gates: bool,
    shamir_inputs: bool,
    inputs_only: bool,
    use_ref: i128,
    user_dir: i128,
    is_real: i128,
    n_requests: usize,
    n_assets: usize,
    user_asset: usize,
    seed: i128,
    policies: bool,
    coefficients: bool,
}

const PROFILES: [Profile; 9] = [
    Profile {
        name: "baseline",
        makers: 4,
        reference: "anchored",
        persist_wires: false,
        binding_limit: false,
        input_check: false,
        audit_gates: false,
        shamir_inputs: false,
        inputs_only: false,
        use_ref: 1,
        user_dir: 0,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 7,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "persist_shamir",
        makers: 16,
        reference: "none",
        persist_wires: true,
        binding_limit: false,
        input_check: false,
        audit_gates: false,
        shamir_inputs: true,
        inputs_only: false,
        use_ref: 1,
        user_dir: 0,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 7,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "binding_check_audit",
        makers: 4,
        reference: "none",
        persist_wires: false,
        binding_limit: true,
        input_check: true,
        audit_gates: true,
        shamir_inputs: false,
        inputs_only: false,
        use_ref: 1,
        user_dir: 0,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 7,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "all_on_shamir",
        makers: 16,
        reference: "anchored",
        persist_wires: true,
        binding_limit: true,
        input_check: true,
        audit_gates: true,
        shamir_inputs: true,
        inputs_only: false,
        use_ref: 1,
        user_dir: 0,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 7,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "padded_non_power",
        makers: 3,
        reference: "anchored",
        persist_wires: false,
        binding_limit: false,
        input_check: true,
        audit_gates: false,
        shamir_inputs: false,
        inputs_only: false,
        use_ref: 1,
        user_dir: 0,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 7,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "sell_cover_no_ref",
        makers: 16,
        reference: "none",
        persist_wires: false,
        binding_limit: false,
        input_check: false,
        audit_gates: true,
        shamir_inputs: true,
        inputs_only: false,
        use_ref: 0,
        user_dir: 1,
        is_real: 0,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 11,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "multi_asset_batch",
        makers: 4,
        reference: "anchored",
        persist_wires: false,
        binding_limit: false,
        input_check: false,
        audit_gates: false,
        shamir_inputs: false,
        inputs_only: false,
        use_ref: 1,
        user_dir: 1,
        is_real: 1,
        n_requests: 2,
        n_assets: 3,
        user_asset: 2,
        seed: 19,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "inputs_only_shamir",
        makers: 3,
        reference: "none",
        persist_wires: true,
        binding_limit: true,
        input_check: true,
        audit_gates: false,
        shamir_inputs: true,
        inputs_only: true,
        use_ref: 0,
        user_dir: 1,
        is_real: 0,
        n_requests: 1,
        n_assets: 1,
        user_asset: 0,
        seed: 23,
        policies: false,
        coefficients: false,
    },
    Profile {
        name: "supplied_policies_coefficients",
        makers: 16,
        reference: "anchored",
        persist_wires: false,
        binding_limit: false,
        input_check: true,
        audit_gates: false,
        shamir_inputs: false,
        inputs_only: false,
        use_ref: 0,
        user_dir: 1,
        is_real: 0,
        n_requests: 1,
        n_assets: 3,
        user_asset: 2,
        seed: 29,
        policies: true,
        coefficients: true,
    },
];

// Recovered from the last exported tree that carried `mp_spdz/gen_qomm.py`.
// Each digest covers exit status, stdout, stderr, and every sorted output
// filename and byte. The former live-oracle test established equality before
// that source was retired.
const RETIRED_GENERATOR_ALL_FILES_CONTRACT: [(&str, usize, usize, &str); 27] = [
    (
        "rfq_baseline",
        9,
        17_683,
        "92a480fd5723a5d5a0a03e057604744705354b52eba72d0a66f97add5a62afed",
    ),
    (
        "rfm_baseline",
        9,
        17_126,
        "6414b98090fc76d0a821e97c70ec87ac53361ac86880e70f5faf94095bf4a100",
    ),
    (
        "rfs_baseline",
        9,
        17_541,
        "6a2d9b91ee13a974e3af63d0dff66e585cc300a85806ef91e7b967968314c801",
    ),
    (
        "rfq_persist_shamir",
        9,
        103_406,
        "630610bb2362e82a99cfbbe2eb1e0a3e0dd30fb30e725c72f2867c3261c9dea3",
    ),
    (
        "rfm_persist_shamir",
        9,
        100_501,
        "423fa3961bf83c5cd767d1c4a63868dc1b757769a4ecacce05d36a7799c67f60",
    ),
    (
        "rfs_persist_shamir",
        9,
        100_916,
        "c6d03b61a3575d7f78d2ea86f02423a986fef6e908f2f61fc3c18e74b42bb236",
    ),
    (
        "rfq_binding_check_audit",
        9,
        24_261,
        "0f8a2bb36353f7ccb898974264f81fc0bc26b44029a3b299ec6fdf04d634a644",
    ),
    (
        "rfm_binding_check_audit",
        9,
        22_976,
        "929b8026aaca223bbe2ccdc4ed3b27e30d2ec7fb0293cb8faa51e15e39bd3b32",
    ),
    (
        "rfs_binding_check_audit",
        9,
        23_391,
        "7358ee7f54c7dbece343bc50e0c42996cb8172d8e2fbe34316291542a16336fe",
    ),
    (
        "rfq_all_on_shamir",
        9,
        110_849,
        "f2d39c1ad3bae42271e0bde20ba8d153104ad900ebef3721aca9fbc6a6d148f7",
    ),
    (
        "rfm_all_on_shamir",
        9,
        107_216,
        "9630991a981c9bb3aa51a5fce7b495294ab188309e1c25536363c8fff4582e7a",
    ),
    (
        "rfs_all_on_shamir",
        9,
        107_631,
        "8c932c6c8165c9108601d3cc592edf37ed0bd90fe25d7dda031e6428d4fa6b44",
    ),
    (
        "rfq_padded_non_power",
        9,
        20_052,
        "feed72ba11e379c8ae31714e2d7356fcc7f21e54b9692a2e4b839c54e85d279f",
    ),
    (
        "rfm_padded_non_power",
        9,
        19_495,
        "5d1fd4eab4261a40c91dec73b1d35465754a5f541f58ac7878c89f17edfe354f",
    ),
    (
        "rfs_padded_non_power",
        9,
        19_910,
        "d5b2b15e76c1b3706f9f8097f6c5c3a5348c8c815e37d63fe574a18677dbcac9",
    ),
    (
        "rfq_sell_cover_no_ref",
        9,
        101_613,
        "7080e8d9734e2553feb003d3b5bb2b168062a5b5af27cf3d4c1a9f0b9475ca18",
    ),
    (
        "rfm_sell_cover_no_ref",
        9,
        101_056,
        "60a7e9df219db86a220de37d1f51eec137f4c1e6a4e98de1a46327a911921e4e",
    ),
    (
        "rfs_sell_cover_no_ref",
        9,
        101_471,
        "13b870bbd5fedda5ecdd5e36c91a04a09e397c57b88df77b5869a6c3f0fd9eaa",
    ),
    (
        "rfq_multi_asset_batch",
        9,
        18_376,
        "19ddbc2bda1f44cb3c6415d8a66ef290f136d49aa534fd617d35bbc801628496",
    ),
    (
        "rfm_multi_asset_batch",
        9,
        17_819,
        "a49bead001d63dae971006aa27a87c84da903032bce88ae045bd0f11f24a0015",
    ),
    (
        "rfs_multi_asset_batch",
        9,
        18_234,
        "f18cc5c8898afba3046de80dd5f84bcb2f5b152c39124b2c346c8e5d96d0d339",
    ),
    (
        "rfq_inputs_only_shamir",
        8,
        28_201,
        "b20a6ab2014339074333f99297fdc2737b75c84c94428a4ae63fb82cfea5187a",
    ),
    (
        "rfm_inputs_only_shamir",
        8,
        28_201,
        "6706cdf37a72d81430f0b787ccf7f5ad9676b911d6f70516b70507dd585563b7",
    ),
    (
        "rfs_inputs_only_shamir",
        8,
        28_201,
        "12a04cf3da9037fac48ad1e34ba49a378951cccd05337491e23d450d6c99e81e",
    ),
    (
        "rfq_supplied_policies_coefficients",
        9,
        41_202,
        "1e0979448f8d191b732ce37dbf795321dbc477bdc09619e0b30e2831f2a6d5ea",
    ),
    (
        "rfm_supplied_policies_coefficients",
        9,
        40_645,
        "f448ebd0e7b147068cba203ca16e5b38586e1a9c85bb9c1ff583947b1b32e5d8",
    ),
    (
        "rfs_supplied_policies_coefficients",
        9,
        41_060,
        "0549bd8528dd4b51beb64c0673e7b4308453263a5167108ac384c4c1180b3dd7",
    ),
];

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("qomm-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn policy_json(count: usize) -> String {
    let entries = (0..count)
        .map(|maker| {
            format!(
                "{{\"asset\":{},\"ask_level\":{},\"spread\":20,\"slope\":1,\"invcoef\":1,\"inv\":{},\"maxqty\":500,\"expiry\":1500,\"active\":1,\"use_ref\":{}}}",
                maker % 3,
                maker as i128 - 8,
                maker as i128 - 4,
                maker % 2
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{entries}]\n")
}

fn common_options(
    profile: Profile,
    mode: &str,
    policies: &Path,
    coefficients: &Path,
) -> Vec<String> {
    let mut options = vec![
        "--mode".into(),
        mode.into(),
        "--n-mm".into(),
        profile.makers.to_string(),
        "--bit-length".into(),
        "31".into(),
        "--reference".into(),
        profile.reference.into(),
        "--use-ref".into(),
        profile.use_ref.to_string(),
        "--user-dir".into(),
        profile.user_dir.to_string(),
        "--is-real".into(),
        profile.is_real.to_string(),
        "--n-requests".into(),
        profile.n_requests.to_string(),
        "--n-assets".into(),
        profile.n_assets.to_string(),
        "--user-asset".into(),
        profile.user_asset.to_string(),
        "--seed".into(),
        profile.seed.to_string(),
    ];
    for (enabled, option) in [
        // Persistence is an RFQ-only output contract.  The Python generator
        // silently ignores this flag for RFM/RFS, whereas the Rust generator
        // deliberately refuses that misleading request.  Keep the byte-parity
        // matrix on valid configurations and test the refusal separately.
        (profile.persist_wires && mode == "rfq", "--persist-wires"),
        (profile.binding_limit, "--binding-limit"),
        (profile.input_check, "--input-check"),
        (profile.audit_gates, "--audit-gates"),
        (profile.inputs_only, "--inputs-only"),
    ] {
        if enabled {
            options.push(option.into());
        }
    }
    if profile.shamir_inputs {
        options.extend([
            "--shamir-inputs".into(),
            "--field-bits".into(),
            "253".into(),
        ]);
    }
    if profile.policies {
        options.extend(["--policies".into(), policies.display().to_string()]);
    }
    if profile.coefficients {
        options.extend([
            "--check-coefficients".into(),
            coefficients.display().to_string(),
        ]);
    }
    if matches!(
        profile.name,
        "inputs_only_shamir" | "supplied_policies_coefficients"
    ) {
        options.extend([
            "--check-mode".into(),
            "aggregate".into(),
            "--unsound-check-for-measurement".into(),
            "--check-repeats".into(),
            "3".into(),
        ]);
    }
    options
}

fn output_options(root: &Path) -> [String; 6] {
    [
        "--out-program".into(),
        root.join("prog.mpc").display().to_string(),
        "--out-input-dir".into(),
        root.display().to_string(),
        "--out-reference".into(),
        root.join("ref.json").display().to_string(),
    ]
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
        .expect("read output directory")
        .map(|entry| {
            let entry = entry.expect("directory entry");
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("read output file"),
            )
        })
        .collect()
}

fn contract_digest(output: &Output, root: &Path) -> (usize, usize, String) {
    let files = files(root);
    let mut digest = Sha256::new();
    digest.update(format!("status={}\0", output.status.code().unwrap_or(-1)));
    digest.update(b"stdout\0");
    digest.update(&output.stdout);
    digest.update(b"\0stderr\0");
    digest.update(&output.stderr);
    digest.update(b"\0");
    let mut bytes = 0;
    for (name, contents) in &files {
        bytes += contents.len();
        digest.update(name.as_bytes());
        digest.update(b"\0");
        digest.update(contents);
        digest.update(b"\0");
    }
    (files.len(), bytes, hex::encode(digest.finalize()))
}

#[test]
fn all_emitted_files_match_the_retired_generator_for_27_cases() {
    let directory = TestDir::new("all-files-parity");
    let rust_generator = env!("CARGO_BIN_EXE_qomm-gen");
    let policies = directory.0.join("policies.json");
    let coefficients = directory.0.join("coefficients.json");
    fs::write(&policies, policy_json(16)).unwrap();
    fs::write(&coefficients, "[2, 5, 9]\n").unwrap();

    let mut cases = 0;
    let mut contracts = RETIRED_GENERATOR_ALL_FILES_CONTRACT.into_iter();
    for profile in PROFILES {
        for mode in ["rfq", "rfm", "rfs"] {
            cases += 1;
            let name = format!("{mode}_{}", profile.name);
            let rust_root = directory.0.join(&name);
            fs::create_dir(&rust_root).unwrap();

            let common = common_options(profile, mode, &policies, &coefficients);
            let rust = Command::new(rust_generator)
                .args(&common)
                .args(output_options(&rust_root))
                .output()
                .expect("run Rust generator");
            assert_success(&format!("Rust case {name}"), &rust);

            let (contract_name, expected_files, expected_bytes, expected_digest) =
                contracts.next().expect("one retired contract per case");
            assert_eq!(name, contract_name);
            let (file_count, bytes, digest) = contract_digest(&rust, &rust_root);
            let expected = 7 + 1 + usize::from(!profile.inputs_only);
            assert_eq!(file_count, expected, "wrong file count for {name}");
            assert_eq!(
                file_count, expected_files,
                "retired file count changed for {name}"
            );
            assert_eq!(
                bytes, expected_bytes,
                "retired byte count changed for {name}"
            );
            assert_eq!(
                digest, expected_digest,
                "retired output bytes changed for {name}"
            );
            println!("CONTRACT\t{name}\t{file_count}\t{bytes}\tIDENTICAL");
        }
    }
    assert_eq!(cases, 27);
    assert!(contracts.next().is_none());
}

#[test]
fn persist_wires_on_non_rfq_is_refused_instead_of_silently_ignored() {
    let rust_generator = env!("CARGO_BIN_EXE_qomm-gen");
    let directory = TestDir::new("non-rfq-persistence-refusal");
    for mode in ["rfm", "rfs"] {
        let root = directory.0.join(mode);
        fs::create_dir(&root).unwrap();
        let output = Command::new(rust_generator)
            .args(["--mode", mode, "--persist-wires"])
            .args(output_options(&root))
            .output()
            .expect("run Rust generator");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert!(stderr.contains("--persist-wires writes wires only on the rfq path"));
        assert!(stderr.contains(&format!("mode is {mode}")));
    }
}

#[test]
fn short_policies_failure_matches_the_retired_generator_contract() {
    let directory = TestDir::new("policy-failure-parity");
    let rust_generator = env!("CARGO_BIN_EXE_qomm-gen");
    let policies = directory.0.join("policies.json");
    fs::write(&policies, policy_json(8)).unwrap();
    let options = [
        "--inputs-only".to_owned(),
        "--n-mm".into(),
        "16".into(),
        "--policies".into(),
        policies.display().to_string(),
    ];
    let rust_root = directory.0.join("rust");
    fs::create_dir(&rust_root).unwrap();
    let rust = Command::new(rust_generator)
        .args(&options)
        .args(output_options(&rust_root))
        .output()
        .unwrap();

    assert_eq!(rust.status.code(), Some(1));
    assert!(rust.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&rust.stderr),
        "--policies has 8 entries for 16 makers\n"
    );
    assert!(files(&rust_root).is_empty());
}
