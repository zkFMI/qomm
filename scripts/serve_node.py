#!/usr/bin/env python3
"""Run one durable QOMM computing-node endpoint from a deployment config."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import signal
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_transport.executor import ProgramRegistry  # noqa: E402
from qomm_transport.node_service import (NodeStore, Principal, ResidentNodeServer,
                                         certificate_fingerprint,
                                         server_ssl_context)  # noqa: E402


def _frame_key(row: dict, base: Path) -> bytes | None:
    if row["role"] != "client":
        return None
    path = (base / row["frame_key_file"]).resolve()
    mode = path.stat().st_mode & 0o777
    if mode & 0o077:
        raise PermissionError(f"frame key {path} must use mode 600")
    value = path.read_bytes()
    if len(value) != 32:
        raise ValueError("frame key file must contain exactly 32 raw bytes")
    return value


async def run(config_path: Path) -> None:
    config = json.loads(config_path.read_text(encoding="utf-8"))
    base = config_path.parent
    principals = {}
    for row in config["principals"]:
        cert = (base / row["certificate_der"]).read_bytes()
        fingerprint = certificate_fingerprint(cert)
        principals[fingerprint] = Principal(row["role"], _frame_key(row, base))
    registry = ProgramRegistry.from_json(config["node"], base / config["program_registry"])
    store = NodeStore(base / config["database"])
    server = ResidentNodeServer(
        config["node"], config["host"], config["port"],
        server_ssl_context(base / config["certificate"], base / config["private_key"],
                           base / config["ca_certificate"]),
        principals, store, compute=registry,
        idle_timeout=float(config.get("idle_timeout_seconds", 30)))
    await server.start()
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for name in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(name, stop.set)
    print(json.dumps({"status": "ready", "node": config["node"],
                      "host": config["host"], "port": server.port}), flush=True)
    await stop.wait()
    await server.stop()
    store.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path, required=True)
    args = parser.parse_args()
    asyncio.run(run(args.config.resolve()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
