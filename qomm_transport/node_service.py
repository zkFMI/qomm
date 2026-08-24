"""Resident, mutually authenticated transport for one MPC computing node.

The service keeps its TLS listener and durable idempotency database alive across
slots.  It accepts exactly one fixed-size frame per authenticated principal and
slot.  A reconnect can repeat the same request identifier safely; changing the
body behind an identifier or replacing a slot frame fails closed.

The computation hook is injected from an allow-listed circuit registry.  This
module deliberately has no "run this shell command" request, because exposing
one at the node boundary would turn the MPC service into remote code execution.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import os
import sqlite3
import ssl
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Awaitable, Callable

from .wire import FRAME_BYTES, Frame, frame_is_authentic

VERSION = 1
RECORD_BYTES = 4096
LENGTH_BYTES = 4
MAX_JSON_BYTES = RECORD_BYTES - LENGTH_BYTES


def certificate_fingerprint(der: bytes) -> str:
    return hashlib.sha256(der).hexdigest()


def server_ssl_context(cert: Path | str, key: Path | str,
                       ca: Path | str) -> ssl.SSLContext:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(str(cert), str(key))
    context.load_verify_locations(cafile=str(ca))
    context.verify_mode = ssl.CERT_REQUIRED
    context.check_hostname = False
    return context


def client_ssl_context(cert: Path | str, key: Path | str,
                       ca: Path | str) -> ssl.SSLContext:
    context = ssl.create_default_context(ssl.Purpose.SERVER_AUTH, cafile=str(ca))
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(str(cert), str(key))
    context.check_hostname = True
    return context


def encode_record(message: dict) -> bytes:
    raw = json.dumps(message, sort_keys=True, separators=(",", ":")).encode()
    if len(raw) > MAX_JSON_BYTES:
        raise ValueError("control message exceeds the fixed record size")
    return len(raw).to_bytes(LENGTH_BYTES, "big") + raw + os.urandom(
        RECORD_BYTES - LENGTH_BYTES - len(raw))


def decode_record(record: bytes) -> dict:
    if len(record) != RECORD_BYTES:
        raise ValueError("control record has the wrong fixed size")
    length = int.from_bytes(record[:LENGTH_BYTES], "big")
    if length > MAX_JSON_BYTES:
        raise ValueError("control record declares an invalid JSON length")
    try:
        message = json.loads(record[LENGTH_BYTES:LENGTH_BYTES + length])
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("control record does not contain valid JSON") from exc
    if not isinstance(message, dict):
        raise ValueError("control message must be a JSON object")
    return message


@dataclass(frozen=True)
class Principal:
    role: str
    frame_key: bytes | None = None

    def __post_init__(self) -> None:
        if self.role not in {"client", "coordinator", "observer"}:
            raise ValueError("unknown node-service role")
        if self.role == "client" and (self.frame_key is None or len(self.frame_key) < 32):
            raise ValueError("client principals need a frame authentication key")


class NodeStore:
    def __init__(self, path: Path | str):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(self.path, isolation_level=None)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS requests (
                principal TEXT NOT NULL,
                request_id TEXT NOT NULL,
                request_digest BLOB NOT NULL,
                response BLOB NOT NULL,
                PRIMARY KEY (principal, request_id)
            );
            CREATE TABLE IF NOT EXISTS frames (
                principal TEXT NOT NULL,
                slot INTEGER NOT NULL,
                frame_digest BLOB NOT NULL,
                frame BLOB NOT NULL,
                received_ns INTEGER NOT NULL,
                PRIMARY KEY (principal, slot)
            );
        """)

    def cached(self, principal: str, request_id: str,
               request_digest: bytes) -> dict | None:
        row = self.db.execute(
            "SELECT request_digest, response FROM requests WHERE principal=? AND request_id=?",
            (principal, request_id)).fetchone()
        if row is None:
            return None
        if row[0] != request_digest:
            raise ValueError("request identifier was reused with a different body")
        return json.loads(row[1])

    def cache(self, principal: str, request_id: str, request_digest: bytes,
              response: dict) -> None:
        self.db.execute(
            "INSERT INTO requests(principal,request_id,request_digest,response) VALUES(?,?,?,?)",
            (principal, request_id, request_digest,
             json.dumps(response, sort_keys=True, separators=(",", ":"))))

    def accept_frame(self, principal: str, slot: int, raw: bytes) -> bool:
        digest = hashlib.sha256(raw).digest()
        row = self.db.execute(
            "SELECT frame_digest FROM frames WHERE principal=? AND slot=?",
            (principal, slot)).fetchone()
        if row is not None:
            if row[0] != digest:
                raise ValueError("principal attempted to replace its frame for this slot")
            return False
        self.db.execute(
            "INSERT INTO frames(principal,slot,frame_digest,frame,received_ns) VALUES(?,?,?,?,?)",
            (principal, slot, digest, raw, time.time_ns()))
        return True

    def frames_for_slot(self, slot: int) -> list[bytes]:
        rows = self.db.execute(
            "SELECT frame FROM frames WHERE slot=? ORDER BY principal", (slot,)).fetchall()
        return [row[0] for row in rows]

    def frame_count(self) -> int:
        return int(self.db.execute("SELECT COUNT(*) FROM frames").fetchone()[0])

    def request_count(self) -> int:
        return int(self.db.execute("SELECT COUNT(*) FROM requests").fetchone()[0])

    def close(self) -> None:
        self.db.close()


ComputeHandler = Callable[[dict], dict | Awaitable[dict]]


class ResidentNodeServer:
    def __init__(self, node: int, host: str, port: int, ssl_context: ssl.SSLContext,
                 principals: dict[str, Principal], store: NodeStore,
                 compute: ComputeHandler | None = None, idle_timeout: float = 30.0,
                 response_delay_ms: float = 0.0):
        self.node = node
        self.host = host
        self.port = port
        self.ssl_context = ssl_context
        self.principals = dict(principals)
        self.store = store
        self.compute = compute
        self.idle_timeout = idle_timeout
        if response_delay_ms < 0:
            raise ValueError("response delay cannot be negative")
        self.response_delay_ms = response_delay_ms
        self.server: asyncio.AbstractServer | None = None

    async def start(self) -> int:
        self.server = await asyncio.start_server(
            self._handle, self.host, self.port, ssl=self.ssl_context,
            ssl_handshake_timeout=10.0)
        self.port = self.server.sockets[0].getsockname()[1]
        return self.port

    async def stop(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()

    def _principal(self, writer: asyncio.StreamWriter) -> tuple[str, Principal]:
        ssl_object = writer.get_extra_info("ssl_object")
        der = ssl_object.getpeercert(binary_form=True) if ssl_object else None
        if not der:
            raise PermissionError("mutual TLS client certificate is required")
        fingerprint = certificate_fingerprint(der)
        principal = self.principals.get(fingerprint)
        if principal is None:
            raise PermissionError("client certificate is not registered")
        return fingerprint, principal

    async def _handle(self, reader: asyncio.StreamReader,
                      writer: asyncio.StreamWriter) -> None:
        try:
            fingerprint, principal = self._principal(writer)
            while True:
                record = await asyncio.wait_for(
                    reader.readexactly(RECORD_BYTES), self.idle_timeout)
                try:
                    request = decode_record(record)
                    response = await self._dispatch(fingerprint, principal, request)
                except Exception as exc:  # fail closed, with no secret values
                    response = {"ok": False, "error": type(exc).__name__,
                                "message": str(exc)[:256]}
                if self.response_delay_ms:
                    await asyncio.sleep(self.response_delay_ms / 1000)
                writer.write(encode_record(response))
                await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionError, asyncio.TimeoutError,
                PermissionError):
            pass
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass

    async def _dispatch(self, fingerprint: str, principal: Principal,
                        request: dict) -> dict:
        if request.get("version") != VERSION:
            raise ValueError("unsupported node-service version")
        request_id = request.get("request_id")
        if not isinstance(request_id, str) or not 1 <= len(request_id) <= 128:
            raise ValueError("request_id must contain 1..128 characters")
        request_digest = hashlib.sha256(
            json.dumps(request, sort_keys=True, separators=(",", ":")).encode()).digest()
        cached = self.store.cached(fingerprint, request_id, request_digest)
        if cached is not None:
            return cached
        operation = request.get("operation")
        if operation == "health":
            response = {"ok": True, "node": self.node, "status": "ready",
                        "version": VERSION}
        elif operation == "submit":
            if principal.role != "client":
                raise PermissionError("only client principals may submit frames")
            try:
                raw = base64.b64decode(request["frame"], validate=True)
            except Exception as exc:
                raise ValueError("frame is not valid base64") from exc
            if len(raw) != FRAME_BYTES:
                raise ValueError("submitted frame has the wrong fixed size")
            frame = Frame.decode(raw)
            if frame.node != self.node or frame.slot != request.get("slot"):
                raise ValueError("submitted frame belongs to another node or slot")
            if not frame_is_authentic(principal.frame_key, frame):
                raise ValueError("submitted frame MAC is invalid")
            inserted = self.store.accept_frame(fingerprint, frame.slot, raw)
            response = {"ok": True, "node": self.node, "slot": frame.slot,
                        "accepted": inserted,
                        "frame_digest": hashlib.sha256(raw).hexdigest()}
        elif operation == "compute":
            if principal.role != "coordinator":
                raise PermissionError("only the coordinator may start a computation")
            if self.compute is None:
                raise RuntimeError("this node has no registered computation handler")
            result = self.compute(request)
            if hasattr(result, "__await__"):
                result = await result
            if not isinstance(result, dict):
                raise RuntimeError("computation handler returned a non-object")
            response = {"ok": True, "node": self.node, **result}
        else:
            raise ValueError("unknown node-service operation")
        self.store.cache(fingerprint, request_id, request_digest, response)
        return response


class ResidentNodeClient:
    """Persistent client with bounded reconnect and caller-owned idempotency IDs."""

    def __init__(self, host: str, port: int, ssl_context: ssl.SSLContext,
                 server_name: str, attempts: int = 3):
        self.host, self.port = host, port
        self.ssl_context, self.server_name = ssl_context, server_name
        self.attempts = attempts
        self.reader: asyncio.StreamReader | None = None
        self.writer: asyncio.StreamWriter | None = None

    async def connect(self) -> None:
        self.reader, self.writer = await asyncio.open_connection(
            self.host, self.port, ssl=self.ssl_context,
            server_hostname=self.server_name, ssl_handshake_timeout=10.0)

    async def close(self) -> None:
        if self.writer is not None:
            self.writer.close()
            try:
                await self.writer.wait_closed()
            except ConnectionError:
                pass
        self.reader = self.writer = None

    async def call(self, request: dict) -> dict:
        record = encode_record(request)
        last = None
        for attempt in range(self.attempts):
            try:
                if self.reader is None or self.writer is None:
                    await self.connect()
                self.writer.write(record)
                await self.writer.drain()
                return decode_record(await self.reader.readexactly(RECORD_BYTES))
            except (ConnectionError, asyncio.IncompleteReadError, ssl.SSLError,
                    BrokenPipeError) as exc:
                last = exc
                await self.close()
                if attempt + 1 < self.attempts:
                    await asyncio.sleep(min(0.05 * 2 ** attempt, 0.5))
        raise ConnectionError("node service remained unavailable after reconnects") from last
