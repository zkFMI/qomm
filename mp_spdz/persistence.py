"""Reading back the shares the circuit kept, and checking they are the answer's.

The quote proof is assembled from shares, and until now those shares reached the
prover by a route of their own: the circuit computed a winner, the prover was
handed values, and nothing said the two were the same numbers. A proof about
numbers that merely agree with the computation is not a proof about the
computation.

`sint.write_to_file` makes the circuit keep each node's share where the prover
reads it. This module is the other end of that: it parses MP-SPDZ's persistence
format and reconstructs, so the identity can be *checked* rather than assumed.

The format was read off the bytes and then confirmed against a known result ---
a circuit whose answer the cleartext reference already gives --- because a new
reader that has not reproduced something known is not a reader anyone should
trust. Three things about it are not obvious and each was found the hard way:
the share is little-endian, it is in Montgomery form, and the evaluation points
are one-based.
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass
from pathlib import Path

# The header, from MP-SPDZ's own writers rather than from inspection:
#
#     "Shamir gfp"            10 bytes, the type name
#     sign                     1 byte, from octetStream::store(bigint)
#     length                   4 bytes little-endian, numBytes(prime)
#     prime                    `length` bytes big-endian
#     montgomery               4 bytes little-endian, from Zp_Data::pack
#
# Two things this file used to get wrong. The 4-byte length was read as the
# *element* width; it is the prime's minimal byte length, and the body's width
# is the GMP limb width. They agree at 128 and 253 bits --- 16 and 32 bytes each
# way --- which is why it worked, and they diverge at, say, a 100-bit prime,
# where the header says 13 and the body strides 16. And the montgomery flag was
# not read at all: every value was divided by R whether or not it was in
# Montgomery form, which turns correct shares into consistent nonsense. They
# still reconstruct, every quorum still agrees, and the number is not the one
# the circuit computed.
NAME_BYTES = 10
SIGN_OFFSET = 10
LENGTH_OFFSET = 11
PRIME_OFFSET = 15
LIMB_BYTES = 8


@dataclass(frozen=True)
class Persisted:
    """One node's file: the field it was written in, and its shares."""

    party: int
    prime: int
    element_bytes: int
    shares: list[int]
    #: Whether the values are in Montgomery form. Read from the header rather
    #: than assumed, because assuming it wrong is silent.
    montgomery: bool = True


def read(path: Path, party: int) -> Persisted:
    raw = Path(path).read_bytes()
    length = int.from_bytes(raw[:8], "little")
    header = raw[8:8 + length]
    if not header.startswith(b"Shamir gfp"):
        raise ValueError(f"{path}: not a Shamir gfp persistence file "
                         f"({header[:16]!r})")
    prime_bytes = int.from_bytes(header[LENGTH_OFFSET:PRIME_OFFSET], "little")
    if not prime_bytes:
        raise ValueError(f"{path}: the header declares a zero-byte prime")
    prime = int.from_bytes(header[PRIME_OFFSET:PRIME_OFFSET + prime_bytes], "big")
    montgomery = bool(int.from_bytes(
        header[PRIME_OFFSET + prime_bytes:PRIME_OFFSET + prime_bytes + 4], "little"))
    # The body strides whole limbs, which is not the same as the prime's byte
    # length once the prime is not a whole number of them.
    limbs = (prime.bit_length() + 8 * LIMB_BYTES - 1) // (8 * LIMB_BYTES)
    element = limbs * LIMB_BYTES
    body = raw[8 + length:]
    if len(body) % element:
        raise ValueError(f"{path}: {len(body)} bytes is not a whole number of "
                         f"{element}-byte shares")
    shares = []
    for i in range(0, len(body), element):
        value = int.from_bytes(body[i:i + element], "little")
        if value >= prime:
            raise ValueError(f"{path}: a share at offset {i} is not reduced; "
                             "the file is not what this reader thinks it is")
        shares.append(value)
    return Persisted(party=party, prime=prime, element_bytes=element,
                     shares=shares, montgomery=montgomery)


def from_montgomery(value: int, prime: int, element_bytes: int) -> int:
    """MP-SPDZ keeps field elements multiplied by R. Divide it back out."""
    r = 1 << (8 * element_bytes)
    return value * pow(r, -1, prime) % prime


def reconstruct(points: list[tuple[int, int]], prime: int) -> int:
    """Lagrange at zero. `points` are (evaluation point, share)."""
    total = 0
    for i, (xi, yi) in enumerate(points):
        numerator, denominator = 1, 1
        for j, (xj, _) in enumerate(points):
            if i == j:
                continue
            numerator = numerator * (-xj) % prime
            denominator = denominator * (xi - xj) % prime
        total = (total + yi * numerator * pow(denominator, -1, prime)) % prime
    return total


def recover(directory: Path, parties: int, threshold: int, index: int = 0) -> int:
    """The `index`-th written value, from any threshold-plus-one of the nodes.

    Every subset has to agree. One that does not means a node kept a share of
    something other than what the circuit computed, which is the thing this
    function exists to notice.
    """
    files = [read(Path(directory) / f"Transactions-P{p}.data", p)
             for p in range(parties)]
    prime = files[0].prime
    if any(f.prime != prime for f in files):
        raise ValueError("the nodes did not write in the same field")
    values = {f.party: (from_montgomery(f.shares[index], prime, f.element_bytes)
                        if f.montgomery else f.shares[index])
              for f in files}

    answers = set()
    for subset in itertools.combinations(sorted(values), threshold + 1):
        # Shamir evaluation points are one-based: party p holds f(p + 1)
        answers.add(reconstruct([(p + 1, values[p]) for p in subset], prime))
    if len(answers) != 1:
        raise ValueError(
            f"the nodes' shares do not agree: {len(answers)} different values "
            "reconstruct from different subsets, so at least one node kept a "
            "share of something the circuit did not compute")
    return answers.pop()


#: The order `gen_qomm.build_program(persist_wires=True)` writes per maker.
#: Both sides name it here so a wire added to one and not the other is a
#: mismatch in one file rather than a silent shift of every later index.
WIRE_NAMES = ("mid", "half", "slope", "invcoef", "inv", "maxqty", "expiry",
              "active", "depth", "skew", "ask", "bid", "fits", "ok", "key",
              "fits_margin", "fresh_margin", "fresh_bit",
              "fits_product", "fresh_product", "both", "gated", "cost")

#: Written once, before the per-maker block.
HEADER_NAMES = ("winner_key", "qty")


def read_wires(directory: Path, parties: int, n_makers: int,
               run: int = -1) -> dict:
    """Every written wire as a share map, without reconstructing any of them.

    `recover` reconstructs, which is right for checking that the nodes agree and
    wrong for handing to a prover: the whole point is that nobody puts the
    shares together. This returns `{name: {party: share}}` plus the winner, in
    the field the circuit wrote, and reconstructs nothing.
    """
    files = [read(Path(directory) / f"Transactions-P{p}.data", p)
             for p in range(parties)]
    prime = files[0].prime
    if any(f.prime != prime for f in files):
        raise ValueError("the nodes did not write in the same field")
    expected = len(HEADER_NAMES) + n_makers * len(WIRE_NAMES)
    # MP-SPDZ *appends* to the persistence file, so a directory that has been
    # run three times holds three blocks and the newest one is last. `run`
    # selects which: -1 is the newest, which is what a proof about the request
    # that just ran needs, and an earlier index is what a *query* wants.
    #
    # A query answered against the live state hands whoever asked the current
    # market, which is what a maker watching the tape would pay for. Answered
    # against a block an hour old it is still what a new entrant needs --- the
    # measured drift over 24 s is 1.01x the dispersion the market already has
    # inside one block --- and worth much less to anyone trading against it. So
    # the accumulation this file does is not only a hazard to read from the
    # front by accident; it is the record the query side runs on.
    #
    # Reading the wrong block by accident is still the hazard it was: the answer
    # reconstructs, every quorum agrees, and it is about a request nobody made.
    # Hence `run` is explicit and `runs_in_file` comes back with the result.
    offsets = {}
    for f in files:
        if len(f.shares) % expected:
            raise ValueError(
                f"party {f.party} wrote {len(f.shares)} values, which is not a "
                f"whole number of {expected}-value runs: one side has a wire "
                "the other does not")
        total = len(f.shares) // expected
        index = run if run >= 0 else total + run
        if not 0 <= index < total:
            raise ValueError(
                f"run {run} of {total} in {directory}: there is no such block")
        offsets[f.party] = index * expected
    if len({len(f.shares) for f in files}) != 1:
        raise ValueError("the nodes wrote different numbers of runs")

    def at(index: int) -> dict:
        return {f.party + 1: (
            from_montgomery(f.shares[offsets[f.party] + index], prime,
                            f.element_bytes)
            if f.montgomery else f.shares[offsets[f.party] + index])
            for f in files}

    makers = []
    for m in range(n_makers):
        base = len(HEADER_NAMES) + m * len(WIRE_NAMES)
        makers.append({name: at(base + k) for k, name in enumerate(WIRE_NAMES)})
    out = {"prime": prime, "makers": makers,
           "runs_in_file": len(files[0].shares) // expected}
    for k, name in enumerate(HEADER_NAMES):
        out[name] = at(k)
    return out
