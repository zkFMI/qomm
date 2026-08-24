#!/usr/bin/env python3
"""Fail-closed experiment launcher with a hash-bound contract and ledger.

The manifest is accepted and its command launched in one process. There is no
separate "prepare" state in which unrecorded diagnostics can be run. Every
finished command receives one verdict and a hash-chained ledger entry.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTRACT = ROOT / "research" / "contract.json"
LEDGER = ROOT / "research" / "ledger.jsonl"
ACTIVE = ROOT / "research" / "active"
REQUIRED = {
    "experiment_id", "contract_id", "contract_sha256", "stage",
    "bottleneck", "hypothesis", "prediction", "likely_failure",
    "single_change", "baseline", "evaluation_population",
    "rejection_rule", "promotion_rule", "evidence_class", "command",
    "output",
}
ALLOWED_VERDICTS = {
    "diagnostic_only", "smoke_only", "inconclusive", "rejected",
    "confirmation_pending", "accepted", "blocked",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain one JSON object")
    return value


def prior_entries() -> list[dict]:
    if not LEDGER.exists():
        return []
    entries = []
    previous = "0" * 64
    for line_number, line in enumerate(LEDGER.read_text(encoding="utf-8").splitlines(), 1):
        entry = json.loads(line)
        digest = entry.pop("entry_sha256", None)
        if entry.get("previous_entry_sha256") != previous:
            raise ValueError(f"ledger hash chain breaks at line {line_number}")
        expected = hashlib.sha256(json.dumps(
            entry, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        if digest != expected:
            raise ValueError(f"ledger entry hash differs at line {line_number}")
        entry["entry_sha256"] = digest
        entries.append(entry)
        previous = digest
    return entries


def append_receipt(receipt: dict, prior: list[dict]) -> None:
    receipt["previous_entry_sha256"] = (
        prior[-1]["entry_sha256"] if prior else "0" * 64)
    encoded = json.dumps(receipt, sort_keys=True, separators=(",", ":"))
    receipt["entry_sha256"] = hashlib.sha256(encoded.encode()).hexdigest()
    LEDGER.parent.mkdir(parents=True, exist_ok=True)
    line = json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n"
    descriptor = os.open(LEDGER, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    try:
        os.write(descriptor, line.encode())
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def validate(manifest_path: Path) -> tuple[dict, dict, list[dict]]:
    contract = load_json(CONTRACT)
    manifest = load_json(manifest_path)
    missing = REQUIRED - manifest.keys()
    if missing:
        raise ValueError(f"manifest is missing: {', '.join(sorted(missing))}")
    if manifest["contract_id"] != contract["contract_id"]:
        raise ValueError("manifest names another research contract")
    if manifest["contract_sha256"] != sha256(CONTRACT):
        raise ValueError("manifest is stale: contract SHA-256 differs")
    if manifest["stage"] != contract["current_stage"]:
        raise ValueError("manifest does not address the earliest unresolved stage")
    if manifest["evidence_class"] not in ALLOWED_VERDICTS - {"accepted", "blocked"}:
        raise ValueError("manifest asks for an invalid predeclared evidence class")
    command = manifest["command"]
    if (not isinstance(command, list) or not command
            or not all(isinstance(item, str) and item for item in command)):
        raise ValueError("manifest command must be a non-empty string array")
    output = (ROOT / manifest["output"]).resolve()
    if ROOT not in output.parents:
        raise ValueError("experiment output must stay inside the QOMM workspace")
    prior = prior_entries()
    if any(row["experiment_id"] == manifest["experiment_id"] for row in prior):
        raise ValueError("experiment identifier already has a terminal receipt")
    return contract, manifest, prior


def run(manifest_path: Path) -> int:
    _, manifest, prior = validate(manifest_path)
    ACTIVE.mkdir(parents=True, exist_ok=True)
    active = ACTIVE / f"{manifest['experiment_id']}.json"
    descriptor = os.open(active, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        os.write(descriptor, manifest_path.read_bytes())
        os.fsync(descriptor)
    finally:
        os.close(descriptor)

    output = (ROOT / manifest["output"]).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    started = time.time_ns()
    completed = None
    error = None
    try:
        completed = subprocess.run(manifest["command"], cwd=ROOT, check=False,
                                   capture_output=True, text=True)
        if completed.returncode != 0:
            error = (completed.stderr or completed.stdout)[-4000:]
        elif not output.is_file():
            error = "declared output was not created"
    except Exception as exc:
        error = f"{type(exc).__name__}: {exc}"

    verdict = manifest["evidence_class"] if error is None else "blocked"
    receipt = {
        "experiment_id": manifest["experiment_id"],
        "contract_id": manifest["contract_id"],
        "contract_sha256": manifest["contract_sha256"],
        "manifest": str(manifest_path.resolve().relative_to(ROOT)),
        "manifest_sha256": sha256(manifest_path),
        "started_at_ns": started,
        "elapsed_seconds": (time.time_ns() - started) / 1_000_000_000,
        "verdict": verdict,
        "return_code": None if completed is None else completed.returncode,
        "output": manifest["output"],
        "output_sha256": sha256(output) if error is None else None,
        "error": error,
    }
    append_receipt(receipt, prior)
    active.unlink(missing_ok=True)
    if error is not None:
        print(error, file=sys.stderr)
        return 1
    print(json.dumps(receipt, sort_keys=True))
    return 0


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: research_run.py MANIFEST.json")
    return run(Path(sys.argv[1]).resolve())


if __name__ == "__main__":
    raise SystemExit(main())
