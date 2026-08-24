#!/usr/bin/env python3
"""Real-socket seven-node mTLS/WAN/restart acceptance run.

Seven independent listeners and seven SQLite stores are used. Configurable
response delays emulate distinct WAN paths. One node is stopped and restarted
against the same database, then the same request IDs are retried to prove
idempotent recovery.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import datetime as dt
import hashlib
import json
import statistics
import sys
import tempfile
import time
from pathlib import Path

from cryptography.hazmat.primitives import serialization

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_transport.key_management import (create_ca,  # noqa: E402
                                           issue_mutual_tls_certificate,
                                           write_tls_bundle)
from qomm_transport.node_service import (NodeStore, Principal, ResidentNodeClient,  # noqa: E402
                                         ResidentNodeServer, certificate_fingerprint,
                                         client_ssl_context, server_ssl_context)
from qomm_transport.wire import Frame, frame_mac, share_request  # noqa: E402


async def run(delays: list[float], slots: int) -> dict:
    started = time.perf_counter()
    root = Path(tempfile.mkdtemp(prefix="qomm-seven-node-"))
    now = dt.datetime.now(dt.UTC)
    ca_key, ca_cert = create_ca("QOMM seven-node acceptance CA", now=now)

    def issue(name):
        key, cert = issue_mutual_tls_certificate(
            ca_key, ca_cert, name, dns_names=(name,),
            ip_addresses=("127.0.0.1",), now=now)
        paths = write_tls_bundle(root / name, name, key, cert, ca_cert)
        return paths, cert

    client_bundle, client_cert = issue("client")
    coordinator_bundle, coordinator_cert = issue("coordinator")
    client_fp = certificate_fingerprint(
        client_cert.public_bytes(serialization.Encoding.DER))
    coordinator_fp = certificate_fingerprint(
        coordinator_cert.public_bytes(serialization.Encoding.DER))
    frame_key = hashlib.sha256(b"qomm-seven-node-frame-key-v1").digest()
    node_bundles = [issue(f"node-{node}")[0] for node in range(7)]
    stores = [NodeStore(root / f"node-{node}.sqlite3") for node in range(7)]
    servers = []

    async def compute(request):
        return {"slot": request["slot"], "transcript_digest": "cd" * 32}

    async def start_node(node):
        key_path, cert_path, ca_path = node_bundles[node]
        server = ResidentNodeServer(
            node, "127.0.0.1", 0,
            server_ssl_context(cert_path, key_path, ca_path),
            {client_fp: Principal("client", frame_key),
             coordinator_fp: Principal("coordinator")},
            stores[node], compute=compute, response_delay_ms=delays[node])
        await server.start()
        return server

    for node in range(7):
        servers.append(await start_node(node))

    client_key, client_cert_path, client_ca = client_bundle
    clients = [ResidentNodeClient(
        "127.0.0.1", servers[node].port,
        client_ssl_context(client_cert_path, client_key, client_ca), f"node-{node}")
        for node in range(7)]
    latencies = []
    accepted = 0
    last_requests = []
    for slot in range(slots):
        values = [3, 100, 0, 42] if slot == slots // 2 else [0, 0, 0, 0]
        payloads = share_request(values, 7)
        requests = []
        for node, payload in enumerate(payloads):
            frame = Frame(slot, node, payload, frame_mac(frame_key, slot, node, payload))
            requests.append({"version": 1, "request_id": f"slot-{slot}-node-{node}",
                             "operation": "submit", "slot": slot,
                             "frame": base64.b64encode(frame.encode()).decode()})
        before = time.perf_counter()
        replies = await asyncio.gather(
            *(clients[node].call(requests[node]) for node in range(7)))
        last_requests = requests
        latencies.append((time.perf_counter() - before) * 1000)
        accepted += sum(bool(reply.get("accepted")) for reply in replies)

    # Kill and recreate node 3. Its listener changes, its durable store does not.
    recovery_started = time.perf_counter()
    await clients[3].close()
    await servers[3].stop()
    servers[3] = await start_node(3)
    clients[3] = ResidentNodeClient(
        "127.0.0.1", servers[3].port,
        client_ssl_context(client_cert_path, client_key, client_ca), "node-3")
    # Retry the exact request identifier and body that was acknowledged before
    # the restart. The response must come from the durable idempotency record.
    retry = last_requests[3]
    frames_before_retry = stores[3].frame_count()
    requests_before_retry = stores[3].request_count()
    recovered = await clients[3].call(retry)
    frames_after_retry = stores[3].frame_count()
    requests_after_retry = stores[3].request_count()
    recovery_ms = (time.perf_counter() - recovery_started) * 1000

    # A coordinator request also traverses all seven authenticated links.
    coord_key, coord_cert_path, coord_ca = coordinator_bundle
    coordinators = [ResidentNodeClient(
        "127.0.0.1", servers[node].port,
        client_ssl_context(coord_cert_path, coord_key, coord_ca), f"node-{node}")
        for node in range(7)]
    compute_replies = await asyncio.gather(*(coordinators[node].call({
        "version": 1, "request_id": "compute-final", "operation": "compute",
        "slot": slots, "shape_digest": "01" * 32}) for node in range(7)))

    for connection in clients + coordinators:
        await connection.close()
    for server in servers:
        await server.stop()
    for store in stores:
        store.close()
    return {
        "nodes": 7,
        "mutual_tls": True,
        "independent_sqlite_stores": 7,
        "slots": slots,
        "frames_accepted": accepted,
        "real_request_slots": 1,
        "frames_per_slot_constant": True,
        "wan_response_delays_ms": delays,
        "slot_wall_ms": {"mean": statistics.mean(latencies),
                         "median": statistics.median(latencies),
                         "max": max(latencies)},
        "restarted_node": 3,
        "recovery_ms": recovery_ms,
        "recovery_retry_ok": bool(recovered.get("ok")),
        "recovery_retry_was_new": (
            frames_after_retry != frames_before_retry
            or requests_after_retry != requests_before_retry),
        "recovery_frame_count_unchanged": frames_after_retry == frames_before_retry,
        "recovery_request_count_unchanged": requests_after_retry == requests_before_retry,
        "compute_quorum_responses": sum(bool(reply.get("ok")) for reply in compute_replies),
        "elapsed_seconds": time.perf_counter() - started,
        "environment": "seven local listeners with configured WAN response delay; not seven sites",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--delays-ms", default="8,13,21,34,55,70,85")
    parser.add_argument("--slots", type=int, default=5)
    parser.add_argument("--out", type=Path,
                        default=ROOT / "artifacts" / "seven_node_cluster.json")
    args = parser.parse_args()
    delays = [float(value) for value in args.delays_ms.split(",")]
    if len(delays) != 7 or args.slots < 2:
        raise SystemExit("exactly seven delays and at least two slots are required")
    result = asyncio.run(run(delays, args.slots))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n",
                        encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
