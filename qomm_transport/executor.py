"""Allow-listed computation adapter for a resident MPC node service."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class RegisteredProgram:
    shape_digest: str
    argv: tuple[str, ...]
    cwd: Path
    executable_sha256: str
    timeout_seconds: float = 60.0

    def verify(self) -> None:
        if len(self.shape_digest) != 64 or len(bytes.fromhex(self.shape_digest)) != 32:
            raise ValueError("shape digest must be a 32-byte hexadecimal digest")
        if not self.argv or not Path(self.argv[0]).is_absolute():
            raise ValueError("registered executable must use an absolute path")
        executable = Path(self.argv[0]).resolve()
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise ValueError("registered executable is absent or not executable")
        actual = hashlib.sha256(executable.read_bytes()).hexdigest()
        if actual != self.executable_sha256:
            raise ValueError("registered executable digest does not match its bytes")
        if not self.cwd.is_dir() or not 0 < self.timeout_seconds <= 3600:
            raise ValueError("registered working directory or timeout is invalid")
        for argument in self.argv[1:]:
            if "{" in argument and argument not in {"{node}", "{slot}"}:
                raise ValueError("only {node} and {slot} placeholders are allowed")


class ProgramRegistry:
    def __init__(self, node: int, programs: list[RegisteredProgram]):
        self.node = node
        self.programs = {}
        for program in programs:
            program.verify()
            if program.shape_digest in self.programs:
                raise ValueError("duplicate registered shape digest")
            self.programs[program.shape_digest] = program

    @classmethod
    def from_json(cls, node: int, path: Path | str) -> "ProgramRegistry":
        data = json.loads(Path(path).read_text(encoding="utf-8"))
        programs = [RegisteredProgram(
            shape_digest=row["shape_digest"], argv=tuple(row["argv"]),
            cwd=Path(row["cwd"]), executable_sha256=row["executable_sha256"],
            timeout_seconds=float(row.get("timeout_seconds", 60)))
            for row in data.get("programs", [])]
        return cls(node, programs)

    def __call__(self, request: dict) -> dict:
        shape = request.get("shape_digest")
        program = self.programs.get(shape)
        if program is None:
            raise PermissionError("shape is not in the approved program registry")
        slot = request.get("slot")
        if not isinstance(slot, int) or not 0 <= slot < 1 << 63:
            raise ValueError("computation slot is invalid")
        argv = [argument.format(node=self.node, slot=slot) for argument in program.argv]
        started = time.perf_counter_ns()
        try:
            result = subprocess.run(
                argv, cwd=program.cwd, capture_output=True, timeout=program.timeout_seconds,
                check=False, env={"PATH": os.environ.get("PATH", "")})
        except subprocess.TimeoutExpired as exc:
            raise TimeoutError("approved computation exceeded its timeout") from exc
        elapsed = time.perf_counter_ns() - started
        if result.returncode != 0:
            raise RuntimeError(
                "approved computation failed; stdout/stderr retained only as digests")
        return {
            "slot": slot,
            "shape_digest": shape,
            "exit_code": result.returncode,
            "elapsed_ns": elapsed,
            "stdout_digest": hashlib.sha256(result.stdout).hexdigest(),
            "stderr_digest": hashlib.sha256(result.stderr).hexdigest(),
        }
