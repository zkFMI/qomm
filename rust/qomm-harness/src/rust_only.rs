//! Fail-closed enforcement of QOMM's single-language ownership boundary.
//!
//! Rust owns product code, experiments, orchestration, and the Avalanche VM.
//! A narrow C/C++ ABI shim and external native libraries remain allowed. The
//! guard deliberately checks the working tree, not Git's index, so an
//! untracked forbidden implementation cannot enter an experiment unnoticed.

use crate::HarnessResult;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub path: PathBuf,
    pub reason: String,
}

fn forbidden_extensions() -> Vec<String> {
    let script = ["p", "y"].concat();
    vec![
        script.clone(),
        format!("{script}i"),
        format!("{script}c"),
        "ipynb".into(),
        "go".into(),
        "sol".into(),
        "mpc".into(),
    ]
}

fn forbidden_content_markers() -> Vec<String> {
    let language = ["p", "y", "t", "h", "o", "n"].concat();
    vec![
        format!("{language}3"),
        ".ipynb".into(),
        format!("var_os(\"{}\")", language.to_ascii_uppercase()),
        "private_clob".into(),
        "run_clob_baseline".into(),
        "continuous_clob_7".into(),
    ]
}

fn contains_command_phrase(content: &str, phrase: &str) -> bool {
    content.match_indices(phrase).any(|(index, _)| {
        index == 0
            || content[..index]
                .chars()
                .next_back()
                .is_some_and(|before| !before.is_ascii_alphanumeric() && before != '_')
    })
}

fn contains_ascii_word(content: &str, word: &str) -> bool {
    content.match_indices(word).any(|(index, _)| {
        let before_is_boundary = index == 0
            || content[..index]
                .chars()
                .next_back()
                .is_some_and(|before| !before.is_ascii_alphanumeric() && before != '_');
        let after = index + word.len();
        let after_is_boundary = after == content.len()
            || content[after..]
                .chars()
                .next()
                .is_some_and(|next| !next.is_ascii_alphanumeric() && next != '_');
        before_is_boundary && after_is_boundary
    })
}

fn skip_directory(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "artifacts" | "vendor" | "node_modules" | ".venv" | "__pycache__"
    )
}

fn content_is_enforced(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if matches!(name, "Makefile" | "Dockerfile") {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "rs" | "sh" | "toml" | "json" | "yaml" | "yml" | "service" | "md" | "tex"
    )
}

fn is_guard_implementation(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|relative| relative == Path::new("rust/qomm-harness/src/rust_only.rs"))
}

fn is_policy_memory(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|relative| relative.starts_with(Path::new(".codex/project-memory")))
}

fn visit(root: &Path, directory: &Path, violations: &mut Vec<Violation>) -> HarnessResult<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !skip_directory(&name) {
                visit(root, &path, violations)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if forbidden_extensions().iter().any(|item| item == &extension) {
            violations.push(Violation {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                reason: format!("project-owned .{extension} implementation is forbidden"),
            });
            continue;
        }
        if !content_is_enforced(&path)
            || is_policy_memory(&path, root)
            || is_guard_implementation(&path, root)
        {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let folded = content.to_ascii_lowercase();
        for marker in forbidden_content_markers() {
            if folded.contains(&marker) {
                violations.push(Violation {
                    path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    reason: format!("forbidden runtime or compatibility marker `{marker}`"),
                });
                break;
            }
        }
        let forbidden_language = ["p", "y", "t", "h", "o", "n"].concat();
        if contains_ascii_word(&folded, &forbidden_language) {
            violations.push(Violation {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                reason: "legacy language implementation or reference marker is forbidden".into(),
            });
        }
        if matches!(name, "go.mod" | "go.sum") {
            violations.push(Violation {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                reason: "project-owned Go module metadata is forbidden".into(),
            });
        }
        if ["go test", "go build", "go run"]
            .iter()
            .any(|phrase| contains_command_phrase(&folded, phrase))
        {
            violations.push(Violation {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                reason: "project-owned Go command is forbidden".into(),
            });
        }
    }
    Ok(())
}

pub fn repository_violations(root: &Path) -> HarnessResult<Vec<Violation>> {
    let root = root.canonicalize()?;
    let mut violations = Vec::new();
    visit(&root, &root, &mut violations)?;
    violations.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(violations)
}

pub fn validate_repository(root: &Path) -> HarnessResult<()> {
    let violations = repository_violations(root)?;
    if violations.is_empty() {
        return Ok(());
    }
    let mut message = String::from(
        "Rust-only project gate failed; remove every project-owned non-Rust implementation or runtime:\n",
    );
    for violation in violations {
        let _ = writeln!(
            message,
            "- {}: {}",
            violation.path.display(),
            violation.reason
        );
    }
    Err(message.into())
}

pub fn validate_experiment_command(command: &[&str]) -> HarnessResult<()> {
    let Some(program) = command.first().copied() else {
        return Err("experiment command is empty".into());
    };
    let normalized = program.replace('\\', "/");
    let basename = normalized.rsplit('/').next().unwrap_or(program);
    let is_cargo = basename == "cargo";
    let is_workspace_binary = normalized.contains("/rust/target/")
        || normalized.starts_with("rust/target/")
        || normalized.starts_with("./rust/target/");
    if !is_cargo && !is_workspace_binary {
        return Err(format!(
            "experiment command must start with cargo or a built Rust workspace binary, got `{program}`"
        )
        .into());
    }
    let joined = command.join(" ").to_ascii_lowercase();
    for marker in forbidden_content_markers() {
        if joined.contains(&marker) {
            return Err(format!("experiment command contains forbidden marker `{marker}`").into());
        }
    }
    Ok(())
}

/// The sole allowed interpreter boundary is the official compiler living in
/// an external MP-SPDZ checkout. It may only be launched by a Rust process,
/// never as the top-level research command accepted above.
pub fn is_official_mpc_compiler(root: &Path, program: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(program) = program.canonicalize() else {
        return false;
    };
    program == root.join(format!("compile.{}", ["p", "y"].concat()))
        && root.join("Compiler/compilerLib.py").is_file()
        && root.join("README.md").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unique_temp_dir;

    #[test]
    fn command_must_enter_through_the_rust_workspace() {
        assert!(validate_experiment_command(&[
            "cargo",
            "run",
            "--manifest-path",
            "rust/Cargo.toml"
        ])
        .is_ok());
        assert!(validate_experiment_command(&["rust/target/release/run_qomm"]).is_ok());
        assert!(validate_experiment_command(&["bash", "scripts/run.sh"]).is_err());
    }

    #[test]
    fn source_extension_gate_is_fail_closed() {
        let root = unique_temp_dir("qomm-rust-only").unwrap();
        fs::write(root.join(format!("legacy.{}", ["p", "y"].concat())), "").unwrap();
        let violations = repository_violations(&root).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path.file_stem().unwrap(), "legacy");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_abi_shims_are_allowed() {
        let root = unique_temp_dir("qomm-rust-only-native").unwrap();
        fs::write(root.join("shim.cpp"), "extern \"C\" int run();\n").unwrap();
        fs::write(root.join("lib.rs"), "unsafe extern \"C\" {}\n").unwrap();
        assert!(repository_violations(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_the_compiler_inside_the_named_external_checkout_is_accepted() {
        let root = unique_temp_dir("qomm-official-compiler").unwrap();
        fs::create_dir_all(root.join("Compiler")).unwrap();
        fs::write(
            root.join(format!("compile.{}", ["p", "y"].concat())),
            "upstream entry point\n",
        )
        .unwrap();
        fs::write(root.join("Compiler/compilerLib.py"), "upstream module\n").unwrap();
        fs::write(root.join("README.md"), "upstream documentation\n").unwrap();
        assert!(is_official_mpc_compiler(
            &root,
            &root.join(format!("compile.{}", ["p", "y"].concat()))
        ));
        assert!(!is_official_mpc_compiler(
            &root,
            &root.join("Compiler/compilerLib.py")
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
