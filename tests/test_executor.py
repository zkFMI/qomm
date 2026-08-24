import hashlib
import sys
from pathlib import Path

import pytest

from qomm_transport.executor import ProgramRegistry, RegisteredProgram


def program(shape="01" * 32, digest=None):
    executable = Path(sys.executable).resolve()
    return RegisteredProgram(
        shape, (str(executable), "-c", "print('ok')"), Path.cwd(),
        digest or hashlib.sha256(executable.read_bytes()).hexdigest(), 5)


def test_only_a_byte_verified_registered_program_runs():
    registry = ProgramRegistry(2, [program()])
    result = registry({"shape_digest": "01" * 32, "slot": 7})
    assert result["exit_code"] == 0 and result["slot"] == 7
    assert result["stdout_digest"] == hashlib.sha256(b"ok\n").hexdigest()


def test_unknown_shape_and_substituted_binary_are_refused():
    registry = ProgramRegistry(2, [program()])
    with pytest.raises(PermissionError, match="approved"):
        registry({"shape_digest": "02" * 32, "slot": 7})
    with pytest.raises(ValueError, match="digest"):
        ProgramRegistry(2, [program(digest="00" * 32)])


def test_request_cannot_inject_arguments_or_paths():
    registry = ProgramRegistry(2, [program()])
    result = registry({"shape_digest": "01" * 32, "slot": 9,
                       "argv": ["/bin/sh", "-c", "false"]})
    assert result["exit_code"] == 0
