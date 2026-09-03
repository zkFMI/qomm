use qomm_harness::rust_only::validate_repository;
use qomm_harness::{repo_root, HarnessResult};
use std::path::PathBuf;

fn main() -> HarnessResult<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(repo_root);
    validate_repository(&root)?;
    println!(
        "Rust-only project gate passed: {}",
        root.canonicalize()?.display()
    );
    Ok(())
}
