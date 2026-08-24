#!/usr/bin/env python3
"""Compile and run one QOMM quote circuit under MP-SPDZ malicious Shamir.

Measures compiler-reported circuit cost and wall-clock latency, optionally
through loopback delay proxies that emulate a fixed one-way inter-node delay.

The runner restores any pre-existing MP-SPDZ player inputs so that repeated
sweeps do not corrupt an existing checkout.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from scripts.hosts import this_host                                          # noqa: E402

HERE = Path(__file__).resolve().parent
QOMM_DIR = HERE.parent

COMPILE_PATTERNS = {
    "integer_bits": re.compile(r"([\d,]+)\s+integer bits"),
    "integer_opens": re.compile(r"([\d,]+)\s+integer opens"),
    "integer_triples": re.compile(r"([\d,]+)\s+integer triples"),
    "vm_rounds": re.compile(r"([\d,]+)\s+virtual machine rounds"),
}
RUNTIME_TIME = re.compile(r"^Time\s*=\s*([\d.]+)\s*seconds", re.M)
RUNTIME_DATA = re.compile(r"Data sent\s*=\s*([\d.]+)\s*MB in ~([\d,]+) rounds", re.M)
RUNTIME_GLOBAL = re.compile(r"Global data sent\s*=\s*([\d.]+)\s*MB", re.M)


def _int(text: str) -> int:
    return int(text.replace(",", ""))


def free_port_block(count: int, start: int = 21000) -> int:
    """Find a base port with `count` consecutive free TCP ports.

    A long sweep leaves thousands of sockets in TIME_WAIT, so a fixed scan order
    starves quickly. The scan starts at a random offset and retries with backoff
    instead of failing the measurement.
    """
    import random

    span = range(start, 60000 - count, 200)
    bases = list(span)
    for attempt in range(6):
        random.shuffle(bases)
        for base in bases:
            ok = True
            for offset in range(count):
                probe = socket.socket()
                try:
                    probe.bind(("127.0.0.1", base + offset))
                except OSError:
                    ok = False
                finally:
                    probe.close()
                if not ok:
                    break
            if ok:
                return base
        time.sleep(5 * (attempt + 1))
    raise RuntimeError("no free port block after retries")


class MPSpdzRun:
    def __init__(self, root: Path, program: str, n_parties: int, threshold: int,
                 binary: str = "malicious-shamir-party.x"):
        self.root = root
        self.binary = binary
        # Only the Shamir binaries take a threshold. A dishonest-majority
        # protocol has one by definition --- n-1 --- and rejects the flag.
        # Which binaries take -T. It is the ShamirMachineSpec family, and
        # `"shamir" in binary` was standing in for that --- which silently
        # excluded atlas-party.x, whose threshold would then have defaulted
        # to (n-1)/2 and measured 3-of-7 while the caller asked for 2.
        self.pass_threshold = any(k in binary for k in ("shamir", "atlas"))
        self.extra_args: list[str] = []
        self.program = program
        self.n_parties = n_parties
        self.threshold = threshold
        self.run_dir = Path(tempfile.mkdtemp(prefix="qomm-"))
        self.backup = self.run_dir / "backup"
        self.backup.mkdir(parents=True)
        self._saved: list[tuple[Path, Path | None]] = []
        self._port_base: int | None = None

    def install(self, source: Path, input_dir: Path) -> None:
        (self.root / "Player-Data").mkdir(exist_ok=True)
        for p in range(self.n_parties):
            target = self.root / "Player-Data" / f"Input-P{p}-0"
            saved = None
            if target.exists():
                saved = self.backup / f"Input-P{p}-0"
                shutil.copy2(target, saved)
            self._saved.append((target, saved))
            shutil.copy2(input_dir / f"Input-P{p}-0", target)
            out = self.root / "Player-Data" / f"Private-Output-P{p}"
            if out.exists():
                out.unlink()
        dest = self.root / "Programs" / "Source" / f"{self.program}.mpc"
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
        self.source_dest = dest

    def compile(self, extra: list[str] | None = None, prime: int | None = None) -> dict:
        # A custom prime makes the MPC field equal to the commitment scalar field,
        # so the shares the nodes already hold can serve as the witness shares of
        # the sigma proofs. It costs a wider field; that cost is measured.
        # MP-SPDZ warns that compiling with -P "activates code that usually
        # isn't the most efficient variant" and to use -F with the prime given
        # only at run time. The first measurement of the matched field used -P
        # at compile time and paid 14.3x the traffic for it.
        field = (["-F", str(prime.bit_length())] if prime else ["-F", "128"])
        cmd = [sys.executable, "./compile.py", *field, *(extra or []), self.program]
        t0 = time.perf_counter()
        proc = subprocess.run(cmd, cwd=self.root, capture_output=True, text=True)
        elapsed = time.perf_counter() - t0
        out = proc.stdout + proc.stderr
        if proc.returncode != 0:
            raise RuntimeError(f"compile failed:\n{out[-4000:]}")
        stats = {"compile_seconds": elapsed}
        for key, pat in COMPILE_PATTERNS.items():
            m = pat.search(out)
            stats[key] = _int(m.group(1)) if m else None
        stats["compile_log"] = out[-2000:]
        return stats

    def _write_host_files(self, actual_base: int, proxy_base: int, delay_ms: float,
                          per_party_ms: list[float] | None = None) -> list[dict]:
        """Host files and proxy plan. `per_party_ms` gives one node its own distance.

        A link between two parties is delayed by the larger of their two
        distances, which is what a route through the further one costs. With the
        list absent every link takes `delay_ms`, which is the uniform case the
        earlier sweeps measured.
        """
        proxies = []
        n = self.n_parties
        for source in range(n):
            lines = []
            for target in range(n):
                uniform = delay_ms == 0 and not per_party_ms
                if uniform or source == target:
                    port = actual_base + target
                else:
                    port = proxy_base + source * n + target
                    link_ms = delay_ms
                    if per_party_ms:
                        link_ms = max(per_party_ms[source], per_party_ms[target])
                    proxies.append({
                        "source": source, "target": target,
                        "listen_port": port, "target_port": actual_base + target,
                        "one_way_delay_ms": link_ms,
                    })
                lines.append(f"127.0.0.1:{port}")
            (self.run_dir / f"hosts-P{source}").write_text("\n".join(lines) + "\n", encoding="utf-8")
        (self.run_dir / "proxy.json").write_text(
            json.dumps({"one_way_delay_ms": delay_ms, "proxies": proxies}), encoding="utf-8")
        return proxies

    def execute(self, delay_ms: float, timeout: float = 1800.0,
                per_party_ms: list[float] | None = None) -> dict:
        n = self.n_parties
        if self._port_base is None:
            self._port_base = free_port_block(n * (n + 2), 21000)
        actual_base = self._port_base
        proxy_base = actual_base + n + 1
        proxies = self._write_host_files(actual_base, proxy_base, delay_ms,
                                         per_party_ms)

        proxy_proc = None
        if proxies:
            ready = self.run_dir / "ready.json"
            proxy_proc = subprocess.Popen(
                [sys.executable, str(HERE / "wan_proxy.py"),
                 "--config", str(self.run_dir / "proxy.json"), "--ready", str(ready)],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            deadline = time.time() + 20
            while not ready.exists() and time.time() < deadline:
                if proxy_proc.poll() is not None:
                    err = proxy_proc.stderr.read().decode()
                    raise RuntimeError(f"proxy died: {err[-2000:]}")
                time.sleep(0.02)
            if not ready.exists():
                raise RuntimeError("proxy did not become ready")

        procs = []
        logs = []
        try:
            t0 = time.perf_counter()
            for party in range(n):
                log = open(self.run_dir / f"party-{party}.log", "w")
                logs.append(log)
                procs.append(subprocess.Popen(
                    [f"./{self.binary}", str(party), self.program,
                     "-N", str(n),
                     *(["-T", str(self.threshold)] if self.pass_threshold else []),
                     *self.extra_args,
                     "-ip", str(self.run_dir / f"hosts-P{party}")],
                    cwd=self.root, stdout=log, stderr=subprocess.STDOUT,
                ))
            failed = False
            for proc in procs:
                try:
                    rc = proc.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    failed = True
                    rc = -9
                if rc != 0:
                    failed = True
            elapsed = time.perf_counter() - t0
        finally:
            for log in logs:
                log.close()
            if proxy_proc is not None:
                proxy_proc.send_signal(signal.SIGTERM)
                try:
                    proxy_proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proxy_proc.kill()

        party_logs = {p: (self.run_dir / f"party-{p}.log").read_text(errors="replace")
                      for p in range(n)}
        combined = "\n".join(f"===== PARTY {p} =====\n{t}" for p, t in party_logs.items())
        p0 = party_logs[0]
        m_time = RUNTIME_TIME.search(p0)
        m_data = RUNTIME_DATA.search(p0)
        m_glob = RUNTIME_GLOBAL.search(p0)
        return {
            "ok": not failed,
            "wall_seconds": elapsed,
            "party0_seconds": float(m_time.group(1)) if m_time else None,
            "party0_mb": float(m_data.group(1)) if m_data else None,
            "party0_rounds": _int(m_data.group(2)) if m_data else None,
            "global_mb": float(m_glob.group(1)) if m_glob else None,
            "log": combined,
        }

    def cleanup(self) -> None:
        for target, saved in self._saved:
            if saved is not None and saved.exists():
                shutil.copy2(saved, target)
            elif target.exists():
                target.unlink()
        for pattern in (f"Programs/Bytecode/{self.program}-*.bc",
                        f"Programs/Schedules/{self.program}.sch",
                        f"Programs/Public-Input/{self.program}"):
            for path in self.root.glob(pattern):
                path.unlink(missing_ok=True)
        if getattr(self, "source_dest", None) is not None:
            self.source_dest.unlink(missing_ok=True)
        shutil.rmtree(self.run_dir, ignore_errors=True)


def unpack_key(key: int, padded_mm: int) -> tuple[int, int]:
    """Recover (cost, market maker index) from the single opened key.

    Floored division is what the packing assumes, so negative costs (a sell,
    where the circuit minimises the negated bid) unpack correctly.
    """
    index = key % padded_mm
    return (key - index) // padded_mm, index


def verify(mode: str, log: str, reference: dict) -> tuple[bool, str]:
    padded = reference["padded_mm"]
    # The circuit opens `key + mask`, which is uniform to everyone but the
    # trader. Unmasking here is the trader's step, not the venue's: the venue
    # never sees the winning price, which is what `reveal_to(0)` gave it.
    mask = reference.get("mask", 0)
    if mode == "rfq":
        m = re.search(r"^QOMM_MASKED_KEY=(-?\d+)", log, re.M)
        if not m:
            return False, "no masked quote in log"
        got = unpack_key(int(m.group(1)) - mask, padded)
        want = (reference["best_cost"], reference["best_mm"])
        return got == want, f"got={got} want={want}"
    if mode == "rfm":
        a = re.search(r"^QOMM_MASKED_ASK=(-?\d+)", log, re.M)
        b = re.search(r"^QOMM_MASKED_BID=(-?\d+)", log, re.M)
        if not a or not b:
            return False, "no two-sided quote in log"
        got_ask = unpack_key(int(a.group(1)) - mask, padded)
        got_bid = unpack_key(int(b.group(1)) - mask, padded)
        want = ((reference["best_ask"], reference["best_ask_mm"]),
                (-reference["best_bid"], reference["best_bid_mm"]))
        got = (got_ask, got_bid)
        return got == want, f"got={got} want={want}"
    if mode == "rfs":
        steps = re.findall(r"^QOMM_RFS_STEP_(\d+)_KEY=(-?\d+)", log, re.M)
        if not steps:
            return False, "no RFS price series in log"
        first = unpack_key(int(steps[0][1]), padded)
        want = (reference["best_cost"], reference["best_mm"])
        return first == want, f"steps={len(steps)} first={first} want={want}"
    return False, "unknown mode"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mp-spdz-root", type=Path,
                    default=Path(os.environ.get("MP_SPDZ_ROOT", "")) if os.environ.get("MP_SPDZ_ROOT") else None)
    ap.add_argument("--n-mm", type=int, default=16)
    ap.add_argument("--n-parties", type=int, default=7)
    ap.add_argument("--threshold", type=int, default=2)
    ap.add_argument("--mode", choices=("rfq", "rfm", "rfs"), default="rfq")
    ap.add_argument("--rfs-steps", type=int, default=5)
    ap.add_argument("--disclose", choices=("none", "threshold"), default="none")
    ap.add_argument("--delay-ms", type=float, default=0.0)
    ap.add_argument("--per-party-ms", type=float, nargs="+", default=None,
                    help="one distance per party; a link takes the larger of its "
                         "two ends. This is what a deployment looks like and a "
                         "single figure is not")
    ap.add_argument("--repeats", type=int, default=3)
    ap.add_argument("--user-qty", type=int, default=100)
    ap.add_argument("--user-dir", type=int, default=0)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--field-bits", type=int, default=128,
                    help="the field the shares are reconstructed in; has to "
                         "match the prime the circuit runs over")
    ap.add_argument("--bit-length", type=int, default=63)
    ap.add_argument("--argmin-arity", type=int, default=2)
    ap.add_argument("--edabit", action="store_true")
    ap.add_argument("--input-check", action="store_true",
                    help="bind the circuit's inputs to the published commitments")
    ap.add_argument("--trunc-pr", action="store_true",
                    help="probabilistic truncation: a comparison mask sized to the "
                         "value plus a statistical gap instead of a field element")
    ap.add_argument("--protocol", default="malicious-shamir-party.x")
    ap.add_argument("--stop-after", default="tournament",
                    choices=("price", "direction", "gates", "tournament"),
                    help="cut the circuit after a named layer. The result does not "
                         "compute a quote, so it is for attributing rounds to layers "
                         "and not for anything that checks an answer.")
    ap.add_argument("--prepare-only", action="store_true",
                    help="generate, install and compile the circuit, then stop with "
                         "everything in place. What runs it afterwards --- the party "
                         "binary, or the Rust runner that links the engine --- is then "
                         "a separate choice about how the result is read, made against "
                         "the same compiled program rather than against a fresh one.")
    ap.add_argument("--is-real", type=int, default=1, choices=(0, 1))
    ap.add_argument("--n-assets", type=int, default=1)
    ap.add_argument("--batch-size", type=int, default=None,
                    help="preprocessing batch size; the default of 10000 far exceeds "
                         "what one quote needs")
    ap.add_argument("--n-requests", type=int, default=1)
    ap.add_argument("--public-maker-assets", action="store_true")
    ap.add_argument("--audit-gates", action="store_true")
    ap.add_argument("--binding-limit", action="store_true",
                    help="the taker commits an acceptance level; a quote at or "
                         "inside it is a trade, so probing costs a fill")
    ap.add_argument("--user-limit", type=int, default=100000)
    ap.add_argument("--check-mode", choices=("aggregate", "per-party"),
                    default="per-party",
                    help="per-party opens one combination per node, so a failing "
                         "check names the node instead of only detecting one. "
                         "`aggregate` is unsound as emitted and the generator "
                         "refuses it without --unsound-check-for-measurement")
    ap.add_argument("--unsound-check-for-measurement", action="store_true",
                    help="pass it through to the generator, for reproducing the "
                         "cost baseline and nothing else")
    ap.add_argument("--file-prep", action="store_true",
                    help="consume preprocessing from files, so the measurement is the "
                         "online phase only")
    ap.add_argument("--user-asset", type=int, default=0)
    ap.add_argument("--prime", type=int, default=None,
                    help="run the MPC over this prime instead of the default field")
    ap.add_argument("--reference", choices=("anchored", "none"),
                    default="anchored",
                    help="whether the price rule adds a reference price at all")
    ap.add_argument("--use-ref", type=int, default=1,
                    help="whether makers anchor on the public reference price")
    ap.add_argument("--persist-wires", action="store_true",
                    help="each node keeps its share of every wire the joint "
                         "prover needs, not only the winner")
    ap.add_argument("--shamir-inputs", action="store_true",
                    help="deal inputs as Shamir shares of the commitment group's "
                         "scalar field, so the shares the nodes feed are the ones "
                         "the policy audit commits to. Implies that prime and "
                         "sets the field width, because a share is a scalar and "
                         "will not fit anything narrower")
    ap.add_argument("--tag", default="")
    ap.add_argument("--out", type=Path, default=None)
    args = ap.parse_args()

    if args.mp_spdz_root is None:
        print("set MP_SPDZ_ROOT or pass --mp-spdz-root", file=sys.stderr)
        return 2
    root = args.mp_spdz_root.resolve()
    if not (root / args.protocol).exists():
        print(f"{args.protocol} missing under {root}", file=sys.stderr)
        return 2

    if args.shamir_inputs:
        # One flag rather than three that have to agree. Setting the prime and
        # the width by hand and getting one of them wrong produces a wrong
        # answer rather than an error, which is the worst kind of knob.
        from mp_spdz.gen_qomm import ED25519_ORDER

        if args.prime not in (None, ED25519_ORDER):
            print("--shamir-inputs fixes the prime; do not also pass --prime",
                  file=sys.stderr)
            return 2
        args.prime = ED25519_ORDER
        args.field_bits = ED25519_ORDER.bit_length()

    work = Path(tempfile.mkdtemp(prefix="qomm-gen-"))
    program = f"qomm_{args.mode}_m{args.n_mm}_{args.disclose}_{os.getpid()}"
    src = work / f"{program}.mpc"
    inputs = work / "inputs"
    ref_path = work / "reference.json"
    gen = subprocess.run([
        sys.executable, str(QOMM_DIR / "mp_spdz" /
                            os.environ.get("QOMM_GENERATOR", "gen_qomm.py")),
        "--n-mm", str(args.n_mm), "--n-parties", str(args.n_parties),
        "--mode", args.mode, "--rfs-steps", str(args.rfs_steps),
        "--disclose", args.disclose, "--user-qty", str(args.user_qty),
        "--user-dir", str(args.user_dir), "--seed", str(args.seed),
        "--field-bits", str(args.field_bits),
        "--is-real", str(args.is_real),
        "--n-assets", str(args.n_assets),
        "--user-asset", str(args.user_asset),
        "--bit-length", str(args.bit_length),
        "--argmin-arity", str(args.argmin_arity),
        "--stop-after", args.stop_after,
        "--check-mode", args.check_mode,
        *(["--unsound-check-for-measurement"]
          if args.unsound_check_for_measurement else []),
        "--user-limit", str(args.user_limit),
        *(["--binding-limit"] if args.binding_limit else []),
        *(["--edabit"] if args.edabit else []),
        *(["--trunc-pr"] if args.trunc_pr else []),
        *(["--input-check"] if args.input_check else []),
        *(["--shamir-inputs"] if args.shamir_inputs else []),
        "--n-requests", str(args.n_requests),
        *(["--public-maker-assets"] if args.public_maker_assets else []),
        *(["--audit-gates"] if args.audit_gates else []),
        *(["--persist-wires"] if args.persist_wires else []),
        "--use-ref", str(args.use_ref),
        "--reference", args.reference,
        "--out-program", str(src), "--out-input-dir", str(inputs),
        "--out-reference", str(ref_path),
    ], capture_output=True, text=True)
    if gen.returncode != 0:
        print(gen.stdout + gen.stderr, file=sys.stderr)
        return 3
    reference = json.loads(ref_path.read_text())

    run = MPSpdzRun(root, program, args.n_parties, args.threshold, args.protocol)
    if args.prime:
        # the field width is compiled in; the prime itself is a run-time
        # argument, which is the variant MP-SPDZ says is the efficient one
        run.extra_args += ["-P", str(args.prime)]
    if args.batch_size:
        run.extra_args += ["-b", str(args.batch_size)]
    if args.file_prep:
        run.extra_args += ["-F"]
    result = {
        "tag": args.tag, "mode": args.mode, "n_mm": args.n_mm,
        "padded_mm": reference["padded_mm"], "n_parties": args.n_parties,
        "threshold": args.threshold, "disclose": args.disclose,
        "rfs_steps": args.rfs_steps if args.mode == "rfs" else None,
        "delay_ms": args.delay_ms, "repeats": args.repeats,
        "host": this_host(),
        "bit_length": args.bit_length, "argmin_arity": args.argmin_arity,
        "edabit": args.edabit, "trunc_pr": args.trunc_pr,
        "input_check": args.input_check, "check_mode": args.check_mode,
        "binding_limit": args.binding_limit,
        "field_bits": args.field_bits, "protocol": args.protocol, "is_real": args.is_real,
        "prime": str(args.prime) if args.prime else None,
        "n_assets": args.n_assets, "user_asset": args.user_asset,
        "batch_size": args.batch_size, "file_prep": args.file_prep,
        "public_maker_assets": args.public_maker_assets, "audit_gates": args.audit_gates,
        "n_requests": args.n_requests,
    }
    try:
        run.install(src, inputs)
        result["circuit"] = run.compile(prime=args.prime)
        if args.prepare_only:
            # Deliberately not cleaned up, and the temporary directory is kept
            # too: the caller needs the reference to check the answer against.
            result["program"] = program
            result["reference"] = str(ref_path)
            result["work_dir"] = str(work)
            result["prepared"] = True
            # No "verified" key: nothing has run, so there is nothing to verify,
            # and a True here would be a claim about an answer never computed.
            text = json.dumps(result, indent=2, sort_keys=True)
            if args.out:
                args.out.parent.mkdir(parents=True, exist_ok=True)
                args.out.write_text(text + "\n", encoding="utf-8")
            print(text)
            return 0
        samples = []
        verified = None
        detail = ""
        for _ in range(args.repeats):
            exec_result = run.execute(args.delay_ms,
                                      per_party_ms=args.per_party_ms)
            if not exec_result["ok"]:
                result["error"] = "party failure"
                result["log_tail"] = exec_result["log"][-4000:]
                break
            ok, detail = verify(args.mode, exec_result["log"], reference)
            verified = ok if verified is None else (verified and ok)
            samples.append({
                "wall_seconds": exec_result["wall_seconds"],
                "party0_seconds": exec_result["party0_seconds"],
                "party0_mb": exec_result["party0_mb"],
                "party0_rounds": exec_result["party0_rounds"],
                "global_mb": exec_result["global_mb"],
            })
        result["samples"] = samples
        result["verified"] = verified
        result["verify_detail"] = detail
        if samples:
            walls = [s["wall_seconds"] for s in samples]
            p0 = [s["party0_seconds"] for s in samples if s["party0_seconds"] is not None]
            result["wall_median"] = statistics.median(walls)
            result["wall_min"] = min(walls)
            result["party0_median"] = statistics.median(p0) if p0 else None
            result["measured_rounds"] = samples[0]["party0_rounds"]
            result["measured_mb"] = samples[0]["party0_mb"]
    finally:
        if not args.prepare_only:
            run.cleanup()
            shutil.rmtree(work, ignore_errors=True)

    text = json.dumps(result, indent=2, sort_keys=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0 if result.get("verified") else 1


if __name__ == "__main__":
    raise SystemExit(main())
