"""Slot-batching relays, one per computing node.

Each relay holds every frame it receives until the slot boundary, shuffles the
batch, and only then forwards it to its node. Two properties follow.

    origin hiding   the node sees a shuffled batch, so the order in which frames
                    arrive at the node carries nothing about who sent them
    single-share    a relay carries one additive share, which is uniform on its
                    own, so a relay that reads everything it forwards still
                    learns nothing about the request

A relay colluding with its own node breaks the second property for that share
alone; all seven would have to collude to reconstruct a request. That is the
same threshold the rest of the design assumes, and it is stated rather than
engineered away.
"""

from __future__ import annotations

import asyncio
import secrets
import time
from dataclasses import dataclass, field

from .wire import FRAME_BYTES, Frame, frame_is_authentic


@dataclass
class Arrival:
    slot: int
    order: int              # position within the batch the node received
    received_at: float
    size: int


@dataclass
class NodeInbox:
    """What an observer sitting on the relay-to-node link would see."""

    node: int
    arrivals: list[Arrival] = field(default_factory=list)
    frames: dict[int, list[Frame]] = field(default_factory=dict)

    def accept(self, slot: int, batch: list[Frame], at: float) -> None:
        self.frames.setdefault(slot, []).extend(batch)
        for order, frame in enumerate(batch):
            self.arrivals.append(Arrival(slot, order, at, FRAME_BYTES))


class Relay:
    """One relay per node per hop. Batches a slot, shuffles it, forwards it.

    Chaining hops is what removes the single point of trust: one relay knows the
    sender's address, the next knows only the previous relay's. An observer has
    to compromise every hop on a path to follow a frame from a user to a node.
    """

    def __init__(self, node: int, inbox: NodeInbox | None = None, rng=None,
                 downstream_port: int | None = None, hop: int = 0,
                 key: bytes | None = None):
        # The key the client MACs with. Optional because a measurement of
        # transport cost does not need one and a deployment does: without it
        # anyone who can reach the port can write a well-formed frame into a
        # slot's batch. `refused` counts what the check rejected, so a relay
        # that is being fed garbage says so rather than only getting slower.
        self.key = key
        self.refused = 0
        self.node = node
        self.inbox = inbox
        self.downstream_port = downstream_port
        self.hop = hop
        self.bytes_out = 0
        self.rng = rng or secrets.SystemRandom()
        self._pending: dict[int, list[Frame]] = {}
        self.server: asyncio.AbstractServer | None = None
        self.port = 0
        self.bytes_in = 0

    async def start(self) -> int:
        self.server = await asyncio.start_server(self._handle, "127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        return self.port

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while True:
                raw = await reader.readexactly(FRAME_BYTES)
                self.bytes_in += len(raw)
                frame = Frame.decode(raw)
                if self.key is not None and not frame_is_authentic(self.key, frame):
                    self.refused += 1
                    continue
                self._pending.setdefault(frame.slot, []).append(frame)
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        finally:
            writer.close()

    async def close_slot(self, slot: int) -> int:
        """Hand the slot's batch on, in an order independent of how it arrived.

        The handoff goes over a socket like any other, so a hop costs what a hop
        costs. Passing batches in memory would have made the cascade look free.
        """
        batch = self._pending.pop(slot, [])
        self.rng.shuffle(batch)
        if self.downstream_port is not None:
            reader, writer = await asyncio.open_connection("127.0.0.1", self.downstream_port)
            for frame in batch:
                writer.write(frame.encode())
            await writer.drain()
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass
            self.bytes_out += FRAME_BYTES * len(batch)
        elif self.inbox is not None:
            self.inbox.accept(slot, batch, time.perf_counter())
        return len(batch)

    async def stop(self) -> None:
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
