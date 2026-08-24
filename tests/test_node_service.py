import asyncio
import base64
import datetime as dt
import hashlib

from cryptography.hazmat.primitives import serialization
from qomm_transport.key_management import (create_ca,
                                           issue_mutual_tls_certificate,
                                           write_tls_bundle)
from qomm_transport.node_service import (NodeStore, Principal,
                                         ResidentNodeClient,
                                         ResidentNodeServer,
                                         certificate_fingerprint,
                                         client_ssl_context,
                                         server_ssl_context)
from qomm_transport.wire import Frame, PAYLOAD_BYTES, frame_mac


def make_pki(tmp_path):
    now = dt.datetime.now(dt.UTC)
    ca_key, ca_cert = create_ca("test-ca", now=now)
    bundles = {}
    for name in ("node-0", "client-0", "coordinator"):
        key, cert = issue_mutual_tls_certificate(
            ca_key, ca_cert, name, dns_names=(name,),
            ip_addresses=("127.0.0.1",), now=now)
        bundles[name] = (*write_tls_bundle(tmp_path / name, name, key, cert, ca_cert), cert)
    return bundles


def raw_frame(slot, key):
    payload = b"p" + bytes(PAYLOAD_BYTES - 1)
    return Frame(slot, 0, payload, frame_mac(key, slot, 0, payload)).encode()


def test_real_mutual_tls_fixed_records_idempotency_and_reconnect(tmp_path):
    async def scenario():
        bundles = make_pki(tmp_path)
        node_key, node_cert, node_ca, _ = bundles["node-0"]
        client_key, client_cert, client_ca, client_x509 = bundles["client-0"]
        coord_key, coord_cert, coord_ca, coord_x509 = bundles["coordinator"]
        frame_key = b"f" * 32
        client_fp = certificate_fingerprint(client_x509.public_bytes(serialization.Encoding.DER))
        coord_fp = certificate_fingerprint(coord_x509.public_bytes(serialization.Encoding.DER))
        store = NodeStore(tmp_path / "node.sqlite3")
        calls = []

        async def compute(request):
            calls.append(request["slot"])
            return {"slot": request["slot"], "transcript_digest": "ab" * 32}

        server = ResidentNodeServer(
            0, "127.0.0.1", 0,
            server_ssl_context(node_cert, node_key, node_ca),
            {client_fp: Principal("client", frame_key),
             coord_fp: Principal("coordinator")}, store, compute=compute)
        port = await server.start()
        client = ResidentNodeClient(
            "127.0.0.1", port,
            client_ssl_context(client_cert, client_key, client_ca), "node-0")
        raw = raw_frame(9, frame_key)
        request = {"version": 1, "request_id": "submit-9", "operation": "submit",
                   "slot": 9, "frame": base64.b64encode(raw).decode()}
        first = await client.call(request)
        second = await client.call(request)
        assert first == second and first["accepted"]
        assert store.frame_count() == 1
        assert store.request_count() == 1
        await client.close()
        # The same object reconnects and the durable response remains identical.
        assert await client.call(request) == first
        await client.close()

        coordinator = ResidentNodeClient(
            "127.0.0.1", port,
            client_ssl_context(coord_cert, coord_key, coord_ca), "node-0")
        job = {"version": 1, "request_id": "compute-9", "operation": "compute",
               "slot": 9, "shape_digest": "01" * 32}
        assert (await coordinator.call(job))["transcript_digest"] == "ab" * 32
        assert (await coordinator.call(job))["transcript_digest"] == "ab" * 32
        assert calls == [9], "idempotent retry ran the computation twice"
        assert store.request_count() == 2
        await coordinator.close()
        await server.stop()
        store.close()

    asyncio.run(scenario())


def test_frame_replacement_and_bad_mac_fail_closed(tmp_path):
    async def scenario():
        bundles = make_pki(tmp_path)
        node_key, node_cert, node_ca, _ = bundles["node-0"]
        client_key, client_cert, client_ca, client_x509 = bundles["client-0"]
        frame_key = b"f" * 32
        fingerprint = certificate_fingerprint(
            client_x509.public_bytes(serialization.Encoding.DER))
        store = NodeStore(tmp_path / "node.sqlite3")
        server = ResidentNodeServer(
            0, "127.0.0.1", 0,
            server_ssl_context(node_cert, node_key, node_ca),
            {fingerprint: Principal("client", frame_key)}, store)
        port = await server.start()
        client = ResidentNodeClient(
            "127.0.0.1", port,
            client_ssl_context(client_cert, client_key, client_ca), "node-0")
        raw = raw_frame(10, frame_key)
        good = {"version": 1, "request_id": "a", "operation": "submit", "slot": 10,
                "frame": base64.b64encode(raw).decode()}
        assert (await client.call(good))["ok"]
        moved = bytearray(raw)
        moved[20] ^= 1
        bad = dict(good, request_id="b", frame=base64.b64encode(moved).decode())
        reply = await client.call(bad)
        assert not reply["ok"] and "MAC" in reply["message"]
        reused = dict(good, frame=base64.b64encode(raw_frame(11, frame_key)).decode())
        reply = await client.call(reused)
        assert not reply["ok"] and "reused" in reply["message"]
        await client.close()
        await server.stop()
        store.close()

    asyncio.run(scenario())
