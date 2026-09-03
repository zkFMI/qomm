use qomm_harness::{write_pretty_json, HarnessResult};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    match run_main() {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run_main() -> HarnessResult<i32> {
    let check = parse_args()?;
    let root = qomm_harness::repo_root();
    let artifacts = root.join("artifacts");
    let manifest = artifacts.join("MANIFEST.json");
    let current = entries(&artifacts)?;
    if !check {
        let payload = json!({"commit": commit(&root), "artifacts": current});
        write_pretty_json(Some(&manifest), &payload)?;
        let empty = current
            .iter()
            .filter(|(_, record)| record["empty"] == true)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let unlabelled = current
            .iter()
            .filter(|(name, record)| name.ends_with(".json") && record.get("host").is_none())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        println!("wrote artifacts/MANIFEST.json: {} artifacts", current.len());
        if !empty.is_empty() {
            println!(
                "  {} carry no rows, so nothing may be quoted from them: {}",
                empty.len(),
                empty.join(", ")
            );
        }
        if !unlabelled.is_empty() {
            println!(
                "  {} carry no host label, having been written before their runner recorded one: {}",
                unlabelled.len(),
                unlabelled.join(", ")
            );
        }
        return Ok(0);
    }
    if !manifest.exists() {
        eprintln!("{} is missing; run `make manifest`.", manifest.display());
        return Ok(1);
    }
    let recorded_value: Value = serde_json::from_slice(&fs::read(&manifest)?)?;
    let recorded = recorded_value["artifacts"]
        .as_object()
        .ok_or("manifest artifacts is not an object")?;
    let mut problems = Vec::new();
    for (name, record) in recorded {
        match current.get(name) {
            None => problems.push(format!("{name}: in the manifest and not in the tree")),
            Some(value) if value["sha256"] != record["sha256"] => {
                problems.push(format!("{name}: changed since the manifest was written"));
            }
            Some(_) => {}
        }
    }
    let recorded_names = recorded.keys().cloned().collect::<BTreeSet<_>>();
    for name in current
        .keys()
        .filter(|name| !recorded_names.contains(*name))
    {
        problems.push(format!("{name}: in the tree and not in the manifest"));
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    if !problems.is_empty() {
        eprintln!(
            "\n{} artifact(s) do not match the manifest. Either re-run the measurement the paper quotes or re-write the manifest, and say which in the commit.",
            problems.len()
        );
        return Ok(1);
    }
    println!("{} artifacts match the manifest", recorded.len());
    Ok(0)
}

fn entries(artifacts: &Path) -> HarnessResult<BTreeMap<String, Value>> {
    let mut paths = Vec::new();
    collect_files(artifacts, &mut paths)?;
    paths.sort();
    let mut found = BTreeMap::new();
    for path in paths {
        let suffix = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(suffix, "pdf" | "png" | "csv")
            || path.file_name().and_then(|value| value.to_str()) == Some("MANIFEST.json")
            || path
                .components()
                .any(|part| matches!(part.as_os_str().to_str(), Some("figures" | "tapes")))
        {
            continue;
        }
        let raw = fs::read(&path)?;
        let mut record = Map::new();
        record.insert("sha256".into(), json!(hex::encode(Sha256::digest(&raw))));
        record.insert("bytes".into(), json!(raw.len()));
        if suffix == "json" {
            match serde_json::from_slice::<Value>(&raw) {
                Ok(loaded) => {
                    if let Some(object) = loaded.as_object() {
                        for field in ["host", "rustc", "runtime", "target", "group"] {
                            if let Some(value) = object.get(field) {
                                record.insert(field.into(), value.clone());
                            }
                        }
                        let rows = ["rows", "scaling", "chains"]
                            .into_iter()
                            .find_map(|key| object.get(key).and_then(Value::as_array));
                        if let Some(rows) = rows {
                            record.insert("rows".into(), json!(rows.len()));
                            if rows.is_empty() {
                                record.insert("empty".into(), json!(true));
                            }
                        }
                    }
                }
                Err(_) => {
                    record.insert("unreadable".into(), json!(true));
                }
            }
        }
        let relative = path.strip_prefix(artifacts)?.to_string_lossy().to_string();
        found.insert(relative, Value::Object(record));
    }
    Ok(found)
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> HarnessResult<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, output)?;
        } else if path.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn commit(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_args() -> HarnessResult<bool> {
    let mut check = false;
    for argument in std::env::args_os().skip(1) {
        match argument.to_string_lossy().as_ref() {
            "--check" => check = true,
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
    }
    Ok(check)
}
