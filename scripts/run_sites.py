#!/usr/bin/env python3
"""Seven parties across as many real sites as there are machines.

The two-site fixture proved the delay proxy against one real link. The claim
that rests on it is that a round waits on its *slowest* link, which is what
makes "one distant node costs 82% of moving all seven" true; and one link is
one point, which a straight line fits by construction.

This runs the same circuit across an arbitrary number of sites. Each site holds
some parties and reaches the others over a real network path.

**The topology, said out loud.** The sites cannot all reach each other directly
--- one is behind a jump host and another is on a tailnet the third cannot
resolve --- so every cross-site link is carried through the machine running this
script. A packet from site A to site B therefore travels A to here to B, and the
round trip on that path is the sum of two real ones rather than the direct one.
That is a real network path and not an emulated delay, and it is not the direct
path between A and B. Both halves matter and the artifact records the per-site
round trip so a reader can add them up.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from scripts import hosts                                      # noqa: E402
from scripts.measure import summarise                          # noqa: E402


@dataclass
class Site:
    label: str
    ssh: str | None          # None means the machine running this script
    root: str
    parties: list[int]

    @classmethod
    def parse(cls, spec: str) -> "Site":
        label, ssh, root, parties = spec.split(":", 3)
        return cls(label=label, ssh=None if ssh == "local" else ssh, root=root,
                   parties=[int(p) for p in parties.split(",")])


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def rtt_ms(ssh_alias: str) -> float | None:
    """Round trip to the site, measured as a TCP handshake.

    Not an ssh round trip: that is dominated by key exchange and authentication,
    which is a second or so and has nothing to do with propagation. The first
    version of this measured that and reported 567 ms for a machine two hops
    away, which is the kind of number that should stop a reader rather than be
    written down.
    """
    got = run(["ssh", "-G", ssh_alias])
    host = port = None
    for line in got.stdout.splitlines():
        if line.startswith("hostname "):
            host = line.split(None, 1)[1].strip()
        elif line.startswith("port "):
            port = line.split(None, 1)[1].strip()
    if not host or not port:
        return None
    got = run([sys.executable, str(HERE / "tcp_rtt.py"), host, port])
    try:
        value = float(got.stdout.strip())
    except ValueError:
        return None
    return None if value != value else round(value, 2)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", action="append", required=True,
                    help="label:ssh_or_local:mp_spdz_root:p1,p2")
    ap.add_argument("--n-mm", type=int, default=16)
    ap.add_argument("--mode", default="rfq")
    ap.add_argument("--bit-length", type=int, default=31)
    ap.add_argument("--base-port", type=int, default=24100)
    ap.add_argument("--repeats", type=int, default=3)
    ap.add_argument("--compile-on",
                    help="label of the site that compiles; defaults to the local one")
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    sites = [Site.parse(s) for s in args.site]
    owned = {p: s for s in sites for p in s.parties}
    parties = sorted(owned)
    if parties != list(range(7)):
        print(f"the sites hold {parties}, and there are seven parties",
              file=sys.stderr)
        return 1
    local = next((s for s in sites if s.ssh is None), None)
    # The machine running this script anchors the tunnels. It does not have to
    # hold parties, and often cannot: a laptop that drives the run may have no
    # MP-SPDZ of its own, and requiring one there would make the third site a
    # build problem rather than a network one.
    builder = next((s for s in sites if s.label == args.compile_on), None) if args.compile_on \
        else local
    if builder is None:
        print(f"no site labelled {args.compile_on!r} to compile on", file=sys.stderr)
        return 1

    work = Path(tempfile.mkdtemp(prefix="qomm-sites-"))
    program = f"qomm_sites_{args.mode}_m{args.n_mm}"
    print(f"== generating {program} ==")
    got = run([sys.executable, str(HERE.parent / "mp_spdz" / "gen_qomm.py"),
               "--n-mm", str(args.n_mm), "--mode", args.mode, "--n-parties", "7",
               "--bit-length", str(args.bit_length),
               "--out-program", str(work / f"{program}.mpc"),
               "--out-input-dir", str(work / "inputs"),
               "--out-reference", str(work / "reference.json")])
    if got.returncode != 0:
        print(got.stderr[-800:], file=sys.stderr)
        return 2
    reference = json.loads((work / "reference.json").read_text())

    print(f"== certificates for seven parties, on {builder.label} ==")
    if builder.ssh is None:
        run(["bash", "Scripts/setup-ssl.sh", "7"], cwd=Path(builder.root).expanduser())
    else:
        got = run(["ssh", builder.ssh,
                   f"cd {builder.root} && ./Scripts/setup-ssl.sh 7 >/dev/null 2>&1 && "
                   f"ls Player-Data/*.pem | wc -l"])
        print(f"  {got.stdout.strip()} certificates")

    print(f"== compiling once, on {builder.label} ==")
    if builder.ssh is None:
        root = Path(builder.root).expanduser()
        shutil.copy(work / f"{program}.mpc",
                    root / "Programs" / "Source" / f"{program}.mpc")
        got = run([sys.executable, "./compile.py", "-F", "128", program], cwd=root)
        compile_out = got.stdout + got.stderr
        failed = got.returncode != 0
    else:
        subprocess.run(["scp", "-q", str(work / f"{program}.mpc"),
                        f"{builder.ssh}:{builder.root}/Programs/Source/"], check=True)
        got = run(["ssh", builder.ssh,
                   f"cd {builder.root} && python3 ./compile.py -F 128 {program}"])
        compile_out = got.stdout + got.stderr
        failed = got.returncode != 0
    if failed:
        print(compile_out[-1500:], file=sys.stderr)
        return 3
    for line in compile_out.splitlines():
        if "rounds" in line or "triples" in line:
            print("  " + line.strip())

    # the certificates come back here too, so every site gets one set
    certs = work / "certs"
    certs.mkdir(exist_ok=True)
    if builder.ssh is None:
        for pattern in ("*.pem", "*.key"):
            for item in (Path(builder.root).expanduser() / "Player-Data").glob(pattern):
                shutil.copy(item, certs)
    else:
        subprocess.run(f"scp -q '{builder.ssh}:{builder.root}/Player-Data/*.pem' "
                       f"'{builder.ssh}:{builder.root}/Player-Data/*.key' {certs}/",
                       shell=True, check=True)

    # the bytecode comes back here so every site gets the same copy
    bytecode = work / "bytecode"
    bytecode.mkdir(exist_ok=True)
    if builder.ssh is None:
        root = Path(builder.root).expanduser()
        shutil.copy(root / "Programs" / "Schedules" / f"{program}.sch", bytecode)
        for item in (root / "Programs" / "Bytecode").glob(f"{program}-*.bc"):
            shutil.copy(item, bytecode)
    else:
        subprocess.run(f"scp -q {builder.ssh}:{builder.root}/Programs/Schedules/"
                       f"{program}.sch {bytecode}/", shell=True, check=True)
        subprocess.run(f"scp -q '{builder.ssh}:{builder.root}/Programs/Bytecode/"
                       f"{program}-*.bc' {bytecode}/", shell=True, check=True)

    remotes = [s for s in sites if s.ssh]
    run_dirs: dict[str, str] = {}
    for site in remotes:
        target = f"{site.root}/../qomm_sites_run_$$"
        got = run(["ssh", site.ssh,
                   f"d=$(mktemp -d {site.root}/../qomm_sites_XXXX) && "
                   f"mkdir -p $d/Programs/Bytecode $d/Programs/Schedules $d/Player-Data && "
                   f"ln -sf {site.root}/malicious-shamir-party.x $d/ && echo $d"])
        if got.returncode != 0:
            print(f"{site.label}: {got.stderr[-300:]}", file=sys.stderr)
            return 4
        target = got.stdout.strip()
        run_dirs[site.label] = target
        print(f"== shipping to {site.label} ({site.ssh}) ==")
        subprocess.run(["scp", "-q", str(bytecode / f"{program}.sch"),
                        f"{site.ssh}:{target}/Programs/Schedules/"], check=True)
        subprocess.run(f"scp -q {bytecode}/{program}-*.bc "
                       f"{site.ssh}:{target}/Programs/Bytecode/", shell=True, check=True)
        # Only the inputs of the parties this site runs. Shipping all seven put
        # every party's share of the request and of every maker's policy on
        # every machine, which any site operator could add up --- a measurement
        # of geographic independence that handed each site the secret.
        for party in site.parties:
            subprocess.run(["scp", "-q", str(work / "inputs" / f"Input-P{party}-0"),
                            f"{site.ssh}:{target}/Player-Data/"], check=True)
        # Every site needs every party's *certificate*, because the parties
        # authenticate each other, and only its own parties' *private keys*.
        # Both used to be shipped: every site held every node's TLS identity and
        # could speak as any of them. The comment below this line used to say
        # "certificate" and the glob said `*`.
        subprocess.run(f"scp -q {certs}/*.pem {site.ssh}:{target}/Player-Data/",
                       shell=True, check=True)
        for party in site.parties:
            key = certs / f"P{party}.key"
            if key.exists():
                subprocess.run(["scp", "-q", str(key),
                                f"{site.ssh}:{target}/Player-Data/"], check=True)
        run(["ssh", site.ssh,
             f"cd {target}/Player-Data && c_rehash . >/dev/null 2>&1 || true"])
        run(["ssh", site.ssh, f"cd {target}/Player-Data && c_rehash . >/dev/null 2>&1 || true"])

    print("== measuring the round trip to each site ==")
    round_trips = {s.label: rtt_ms(s.ssh) for s in remotes}
    for label, value in round_trips.items():
        print(f"  {label}: {value} ms")

    base = args.base_port
    hosts_file = work / "hosts"
    hosts_file.write_text("".join(f"127.0.0.1:{base + p}\n" for p in parties))
    if local is not None:
        shutil.copy(hosts_file, Path(local.root).expanduser() / "qomm_sites_hosts")
    for site in remotes:
        subprocess.run(["scp", "-q", str(hosts_file),
                        f"{site.ssh}:{run_dirs[site.label]}/qomm_hosts"], check=True)

    print("== opening tunnels ==")
    tunnels = []
    for site in remotes:
        forwards = []
        for p in site.parties:
            forwards += ["-L", f"127.0.0.1:{base + p}:127.0.0.1:{base + p}"]
        for p in parties:
            if p not in site.parties:
                forwards += ["-R", f"127.0.0.1:{base + p}:127.0.0.1:{base + p}"]
        tunnels.append(subprocess.Popen(
            ["ssh", "-N", "-o", "ExitOnForwardFailure=yes",
             "-o", "ServerAliveInterval=15", *forwards, site.ssh]))
    time.sleep(5)
    for site, tunnel in zip(remotes, tunnels):
        if tunnel.poll() is not None:
            print(f"the tunnel to {site.label} did not open", file=sys.stderr)
            for t in tunnels:
                t.terminate()
            return 5

    samples = []
    try:
        for attempt in range(1, args.repeats + 1):
            print(f"== run {attempt} ==")
            started = time.perf_counter()
            procs = []
            # Party 0 is the coordinator every other party dials, so it goes up
            # first and the rest follow. Starting them together made every peer
            # refuse the connections of every other peer, which reads as a
            # network fault and is a starting order.
            ordered = sorted(((s, p) for s in sites for p in s.parties),
                             key=lambda pair: pair[1])
            for site, p in ordered:
                if True:
                    if site.ssh is None:
                        procs.append(subprocess.Popen(
                            ["./malicious-shamir-party.x", str(p), program,
                             "-N", "7", "-T", "2", "-ip", "qomm_sites_hosts"],
                            cwd=Path(site.root).expanduser(),
                            stdout=open(work / f"party-{p}.log", "w"),
                            stderr=subprocess.STDOUT))
                    else:
                        procs.append(subprocess.Popen(
                            ["ssh", site.ssh,
                             f"cd {run_dirs[site.label]} && ./malicious-shamir-party.x "
                             f"{p} {program} -N 7 -T 2 -ip qomm_hosts"],
                            stdout=open(work / f"party-{p}.log", "w"),
                            stderr=subprocess.STDOUT))
                if p == 0:
                    time.sleep(3)
            status = 0
            for proc in procs:
                if proc.wait() != 0:
                    status = 1
            elapsed = time.perf_counter() - started
            print(f"  {elapsed:.3f} s status={status}")
            samples.append(elapsed)
    finally:
        for tunnel in tunnels:
            tunnel.terminate()
        for site in remotes:
            run(["ssh", site.ssh, f"rm -rf {run_dirs[site.label]}"])

    logs = "\n".join((work / f"party-{p}.log").read_text(errors="replace")
                     for p in parties)
    if "QOMM_MASKED_KEY=" not in logs:
        print("== no quote in the logs; the first lines each party wrote ==")
        for p in parties:
            first = (work / f"party-{p}.log").read_text(errors="replace").strip()
            print(f"  party {p}: {first.splitlines()[0] if first else '(nothing)'}")
    masked = None
    for line in logs.splitlines():
        if line.startswith("QOMM_MASKED_KEY="):
            masked = int(line.split("=", 1)[1])
    padded = reference["padded_mm"]
    verified = False
    detail = "no masked quote in the logs"
    if masked is not None:
        key = masked - reference["mask"]
        got = ((key - key % padded) // padded, key % padded)
        want = (reference["best_cost"], reference["best_mm"])
        verified, detail = got == want, f"got={got} want={want}"

    # The engine's own time, per party, which is what the two-site artifact
    # records and what the wall figure cannot be compared against: the wall
    # includes ssh process startup and the deliberate stagger before party zero.
    engine = []
    for p in parties:
        for line in (work / f"party-{p}.log").read_text(errors="replace").splitlines():
            if line.startswith("Time = "):
                engine.append(float(line.split("=", 1)[1].split()[0]))
    rounds = None
    bytes_sent = None
    for line in logs.splitlines():
        if "Data sent" in line and bytes_sent is None:
            bytes_sent = line.strip()
        if "Global data sent" in line:
            bytes_sent = line.strip()

    payload = {
        "host": hosts.this_host(),
        "engine_seconds": summarise(engine) if engine else None,
        "data_sent": bytes_sent,
        "fixture": "seven parties across real sites, cross-site links carried "
                   "through the machine running this script",
        "sites": [{"label": s.label, "parties": s.parties,
                   "round_trip_ms": round_trips.get(s.label),
                   "remote": s.ssh is not None} for s in sites],
        "n_mm": args.n_mm, "mode": args.mode, "bit_length": args.bit_length,
        "wall_seconds": summarise(samples),
        "verified": verified, "verify_detail": detail,
        "claim_boundary": "a link between two remote sites is the sum of two "
                          "real round trips, not the direct one between them, "
                          "because the sites cannot all reach each other",
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(f"wrote {args.out}  verified={verified}  {detail}")
    return 0 if verified else 6


if __name__ == "__main__":
    raise SystemExit(main())
