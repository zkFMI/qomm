//! Rust port of `scripts/scrub_artifacts.py`.

use qomm_harness::HarnessResult;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let (root, apply) = parse_args()?;
    let labels = qomm_measure::hosts::labels();
    let mut names = labels.keys().cloned().collect::<Vec<_>>();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    let mut paths = Vec::new();
    collect_files(&root, &mut paths)?;
    paths.sort();
    let mut total_files = 0;
    let mut total_hits = 0;
    for path in paths {
        let hits = scrub(&path, !apply, &labels, &names)?;
        if hits != 0 {
            total_files += 1;
            total_hits += hits;
            println!("  {}: {hits}", path.strip_prefix(&root)?.display());
        }
    }
    let verb = if apply { "rewrote" } else { "would rewrite" };
    println!("{verb} {total_hits} name(s) across {total_files} file(s)");
    if !apply {
        println!("re-run with --apply to make the change");
    }
    Ok(())
}

fn scrub(
    path: &Path,
    dry_run: bool,
    labels: &BTreeMap<String, String>,
    names: &[String],
) -> HarnessResult<usize> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(_) => return Ok(0),
    };
    let text = match String::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => return Ok(0),
    };
    if names.is_empty() {
        return Ok(0);
    }
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    let mut hits = 0;
    while index < text.len() {
        let remainder = &text[index..];
        if let Some(name) = names
            .iter()
            .find(|name| remainder.starts_with(name.as_str()))
        {
            output.push_str(&labels[name]);
            index += name.len();
            hits += 1;
        } else {
            let ch = remainder
                .chars()
                .next()
                .expect("index is below text length");
            output.push(ch);
            index += ch.len_utf8();
        }
    }
    if hits != 0 && !dry_run {
        fs::write(path, output)?;
    }
    Ok(hits)
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

fn parse_args() -> HarnessResult<(PathBuf, bool)> {
    let mut root = qomm_harness::repo_root().join("artifacts");
    let mut apply = false;
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--root" => {
                index += 1;
                root = PathBuf::from(raw.get(index).ok_or("--root expects a value")?);
            }
            "--apply" => apply = true,
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    Ok((root, apply))
}
