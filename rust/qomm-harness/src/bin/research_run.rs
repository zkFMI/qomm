//! Fail-closed experiment launcher with a hash-bound contract and ledger.

use qomm_harness::rust_only::{validate_experiment_command, validate_repository};
use qomm_harness::{repo_root, HarnessResult};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

type ValidatedResearch = (Map<String, Value>, Vec<Map<String, Value>>);

const REQUIRED: [&str; 16] = [
    "experiment_id",
    "contract_id",
    "contract_sha256",
    "stage",
    "bottleneck",
    "hypothesis",
    "prediction",
    "likely_failure",
    "single_change",
    "baseline",
    "evaluation_population",
    "rejection_rule",
    "promotion_rule",
    "evidence_class",
    "command",
    "output",
];

const ALLOWED_VERDICTS: [&str; 7] = [
    "diagnostic_only",
    "smoke_only",
    "inconclusive",
    "rejected",
    "confirmation_pending",
    "accepted",
    "blocked",
];

struct Paths {
    root: PathBuf,
    contract: PathBuf,
    ledger: PathBuf,
    active: PathBuf,
}

impl Paths {
    fn discover() -> HarnessResult<Self> {
        let root = std::env::var_os("QOMM_RESEARCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(repo_root)
            .canonicalize()?;
        Ok(Self {
            contract: root.join("research/contract.json"),
            ledger: root.join("research/ledger.jsonl"),
            active: root.join("research/active"),
            root,
        })
    }
}

fn sha256(path: &Path) -> HarnessResult<String> {
    Ok(hex::encode(Sha256::digest(fs::read(path)?)))
}

fn load_json(path: &Path) -> HarnessResult<Map<String, Value>> {
    let value: Value = serde_json::from_slice(&fs::read(path)?)?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must contain one JSON object", path.display()).into())
}

fn canonical_json(value: &Value) -> String {
    render_json(value, false)
}

fn spaced_json(value: &Value) -> String {
    render_json(value, true)
}

fn render_json(value: &Value, spaces: bool) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).expect("JSON scalar serializes")
        }
        Value::Array(values) => {
            let separator = if spaces { ", " } else { "," };
            format!(
                "[{}]",
                values
                    .iter()
                    .map(|value| render_json(value, spaces))
                    .collect::<Vec<_>>()
                    .join(separator)
            )
        }
        Value::Object(values) => {
            let item_separator = if spaces { ", " } else { "," };
            let key_separator = if spaces { ": " } else { ":" };
            format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!(
                        "{}{key_separator}{}",
                        serde_json::to_string(key).expect("JSON key serializes"),
                        render_json(value, spaces)
                    ))
                    .collect::<Vec<_>>()
                    .join(item_separator)
            )
        }
    }
}

fn prior_entries(paths: &Paths) -> HarnessResult<Vec<Map<String, Value>>> {
    if !paths.ledger.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    let mut previous = "0".repeat(64);
    for (line_number, line) in fs::read_to_string(&paths.ledger)?.lines().enumerate() {
        let mut entry = serde_json::from_str::<Value>(line)?
            .as_object()
            .cloned()
            .ok_or_else(|| format!("ledger line {} is not an object", line_number + 1))?;
        let digest = entry
            .remove("entry_sha256")
            .and_then(|value| value.as_str().map(str::to_owned));
        if entry.get("previous_entry_sha256").and_then(Value::as_str) != Some(previous.as_str()) {
            return Err(format!("ledger hash chain breaks at line {}", line_number + 1).into());
        }
        let expected = hex::encode(Sha256::digest(canonical_json(&Value::Object(
            entry.clone(),
        ))));
        if digest.as_deref() != Some(expected.as_str()) {
            return Err(format!("ledger entry hash differs at line {}", line_number + 1).into());
        }
        entry.insert("entry_sha256".into(), Value::String(expected.clone()));
        entries.push(entry);
        previous = expected;
    }
    Ok(entries)
}

fn append_receipt(
    paths: &Paths,
    receipt: &mut Map<String, Value>,
    prior: &[Map<String, Value>],
) -> HarnessResult<()> {
    let previous = prior
        .last()
        .and_then(|entry| entry.get("entry_sha256"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "0".repeat(64));
    receipt.insert("previous_entry_sha256".into(), Value::String(previous));
    let encoded = canonical_json(&Value::Object(receipt.clone()));
    let digest = hex::encode(Sha256::digest(encoded));
    receipt.insert("entry_sha256".into(), Value::String(digest));
    if let Some(parent) = paths.ledger.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o644)
        .open(&paths.ledger)?;
    writeln!(file, "{}", canonical_json(&Value::Object(receipt.clone())))?;
    file.sync_all()?;
    Ok(())
}

fn resolve_manifest(argument: &Path) -> HarnessResult<PathBuf> {
    if argument.is_absolute() {
        Ok(argument.canonicalize()?)
    } else {
        Ok(std::env::current_dir()?.join(argument).canonicalize()?)
    }
}

fn resolve_output(root: &Path, raw: &str) -> HarnessResult<PathBuf> {
    let relative = Path::new(raw);
    if relative.is_absolute() {
        return Err("experiment output must stay inside the QOMM workspace".into());
    }
    let mut output = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                if output == root || !output.pop() {
                    return Err("experiment output must stay inside the QOMM workspace".into());
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("experiment output must stay inside the QOMM workspace".into());
            }
        }
    }
    if !output.starts_with(root) || output == root {
        return Err("experiment output must stay inside the QOMM workspace".into());
    }
    Ok(output)
}

fn validate(paths: &Paths, manifest_path: &Path) -> HarnessResult<ValidatedResearch> {
    validate_repository(&paths.root)?;
    let contract = load_json(&paths.contract)?;
    let manifest = load_json(manifest_path)?;
    let missing = REQUIRED
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .difference(&manifest.keys().map(String::as_str).collect::<BTreeSet<_>>())
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("manifest is missing: {}", missing.join(", ")).into());
    }
    if manifest.get("contract_id") != contract.get("contract_id") {
        return Err("manifest names another research contract".into());
    }
    let contract_sha256 = sha256(&paths.contract)?;
    if manifest.get("contract_sha256").and_then(Value::as_str) != Some(contract_sha256.as_str()) {
        return Err("manifest is stale: contract SHA-256 differs".into());
    }
    if manifest.get("stage") != contract.get("current_stage") {
        return Err("manifest does not address the earliest unresolved stage".into());
    }
    let evidence = manifest
        .get("evidence_class")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !ALLOWED_VERDICTS[..5].contains(&evidence) {
        return Err("manifest asks for an invalid predeclared evidence class".into());
    }
    let command = manifest.get("command").and_then(Value::as_array);
    if command.is_none_or(|command| {
        command.is_empty()
            || command
                .iter()
                .any(|item| item.as_str().is_none_or(str::is_empty))
    }) {
        return Err("manifest command must be a non-empty string array".into());
    }
    let command = command.expect("command was just validated");
    validate_experiment_command(
        &command
            .iter()
            .map(|item| item.as_str().expect("command item was just validated"))
            .collect::<Vec<_>>(),
    )?;
    let raw_output = manifest
        .get("output")
        .and_then(Value::as_str)
        .ok_or("manifest output must be a string")?;
    resolve_output(&paths.root, raw_output)?;
    let prior = prior_entries(paths)?;
    let experiment_id = manifest
        .get("experiment_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if prior
        .iter()
        .any(|row| row.get("experiment_id").and_then(Value::as_str) == Some(experiment_id))
    {
        return Err("experiment identifier already has a terminal receipt".into());
    }
    Ok((manifest, prior))
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn run(paths: &Paths, manifest_path: &Path) -> HarnessResult<i32> {
    let (manifest, prior) = validate(paths, manifest_path)?;
    fs::create_dir_all(&paths.active)?;
    let experiment_id = manifest["experiment_id"]
        .as_str()
        .ok_or("experiment_id must be a string")?;
    let active = paths.active.join(format!("{experiment_id}.json"));
    let mut active_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&active)?;
    active_file.write_all(&fs::read(manifest_path)?)?;
    active_file.sync_all()?;
    drop(active_file);

    let raw_output = manifest["output"]
        .as_str()
        .ok_or("manifest output must be a string")?;
    let output = resolve_output(&paths.root, raw_output)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let started = unix_nanos();
    let command = manifest["command"]
        .as_array()
        .expect("command was validated")
        .iter()
        .map(|value| value.as_str().expect("command item was validated"))
        .collect::<Vec<_>>();
    let mut return_code = None;
    let mut error = None::<String>;
    match Command::new(command[0])
        .args(&command[1..])
        .current_dir(&paths.root)
        .output()
    {
        Ok(completed) => {
            return_code = completed.status.code();
            if !completed.status.success() {
                let stderr = String::from_utf8_lossy(&completed.stderr);
                let stdout = String::from_utf8_lossy(&completed.stdout);
                error = Some(tail(
                    if stderr.is_empty() { &stdout } else { &stderr },
                    4_000,
                ));
            } else if !output.is_file() {
                error = Some("declared output was not created".into());
            }
        }
        Err(failure) => error = Some(format!("{}: {failure}", io_error_name(&failure))),
    }
    let finished = unix_nanos();
    let verdict = if error.is_none() {
        manifest["evidence_class"].clone()
    } else {
        Value::String("blocked".into())
    };
    let mut receipt = Map::from_iter([
        ("experiment_id".into(), manifest["experiment_id"].clone()),
        ("contract_id".into(), manifest["contract_id"].clone()),
        (
            "contract_sha256".into(),
            manifest["contract_sha256"].clone(),
        ),
        (
            "manifest".into(),
            Value::String(
                manifest_path
                    .strip_prefix(&paths.root)?
                    .to_string_lossy()
                    .into_owned(),
            ),
        ),
        (
            "manifest_sha256".into(),
            Value::String(sha256(manifest_path)?),
        ),
        ("started_at_ns".into(), Value::from(started as u64)),
        (
            "elapsed_seconds".into(),
            Value::from((finished - started) as f64 / 1_000_000_000.0),
        ),
        ("verdict".into(), verdict),
        (
            "return_code".into(),
            return_code.map_or(Value::Null, Value::from),
        ),
        ("output".into(), manifest["output"].clone()),
        (
            "output_sha256".into(),
            if error.is_none() {
                Value::String(sha256(&output)?)
            } else {
                Value::Null
            },
        ),
        (
            "error".into(),
            error.clone().map_or(Value::Null, Value::String),
        ),
    ]);
    append_receipt(paths, &mut receipt, &prior)?;
    match fs::remove_file(&active) {
        Ok(()) => {}
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure.into()),
    }
    if let Some(error) = error {
        eprintln!("{error}");
        return Ok(1);
    }
    println!("{}", spaced_json(&Value::Object(receipt)));
    Ok(0)
}

fn io_error_name(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

fn tail(text: &str, chars: usize) -> String {
    let values = text.chars().collect::<Vec<_>>();
    values[values.len().saturating_sub(chars)..]
        .iter()
        .collect()
}

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        eprintln!("usage: research_run MANIFEST.json");
        std::process::exit(1);
    }
    let result = (|| -> HarnessResult<i32> {
        let paths = Paths::discover()?;
        let manifest = resolve_manifest(Path::new(&args[0]))?;
        run(&paths, &manifest)
    })();
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("research_run: {error}");
            std::process::exit(1);
        }
    }
}
