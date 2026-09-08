# QOMM / DeFMI Docker demo network

This directory holds the start-up definition of the public demo
(`zkfmi.com`, port 8800) so that it can be rebuilt from this repository alone.
The product source stays outside it; the build files here only assemble it.

The compose project starts these independent services:

- `defmi-network`: five unmodified AvalancheGo validators running the Rust
  DeFMI VM (`qomm-avalanche-vm`) behind one stable `/rpc` endpoint, launched by
  `qomm-defmi-network` (the `defmi_network` binary from `zkFMI/defmi`).
- `mpc-0` .. `mpc-6`: one MP-SPDZ party per container. Each receives only its
  own share of every input and runs the stock `malicious-shamir-party.x`
  from the hybrid-TLS build described below.
- `maker-0` .. `maker-3`, `taker`: the participant module of each Maker and the
  Taker. Each keeps its five purpose-specific keys encrypted in its own volume
  and refuses to sign anything after the fill.
- `mpc-operator-0` .. `mpc-operator-6`: the participant modules of the seven
  operators, in separate containers from the parties they operate; they
  register the operator/node binding on DeFMI.
- `gateway`: the Rust service the browser talks to; it dispatches every round
  to the seven MPC services (`--engine distributed`) and never runs a local
  substitute.
- `frontend`: serves the static UI (`qomm_demo/static/`) and proxies `/ws` to
  the gateway inside the compose network.

All services share one bridge network. Only the browser port `8800` and the
DeFMI `/rpc` port `9650` are published to the host; the MPC, participant and
gateway (`8801`) APIs stay internal.

## Pinned inputs

The image is built from this repository checkout only. Everything it needs
from other repositories is fetched at a fixed revision inside the build:

| Input | Where pinned | Value |
| --- | --- | --- |
| Rust crates from `zkFMI/*` | `rust/Cargo.toml` (`rev =`), `rust/Cargo.lock` | built with `--locked` |
| `zkFMI/defmi` (VM, launcher, genesis) | `ARG DEFMI_COMMIT` in `Dockerfile` | `1034c0a76c76a88e266ae6afef8a7ce4607c6f1f` |
| `zkFMI/zkpi` (MP-SPDZ patches) | `ARG ZKPI_COMMIT` in `Dockerfile` | `38baa1a86bedd2b47d7439c57412d25a20f9c405` |
| `data61/MP-SPDZ` | `ARG MP_SPDZ_COMMIT` in `Dockerfile` | `9d809599ea6ce627216a389ca7d984fbb75d0cb9` |
| OpenSSL | `ARG OPENSSL_VERSION` / `OPENSSL_SHA256` | `3.5.5` |
| AvalancheGo / network runner | `avalanche-download` stage | `1.14.2` / `1.8.3`, sha256 per architecture |
| Rust toolchain | `ARG RUST_VERSION` | `1.97.1` |

`DEFMI_COMMIT` and `ZKPI_COMMIT` must equal the `rev` of `defmi` and `zkpi` in
`rust/Cargo.toml`; when those move, update the two build arguments in the same
change.

## Hybrid TLS between the MPC parties

The MP-SPDZ engine is not the stock build. The `mp-spdz-builder` stage:

1. builds OpenSSL 3.5.5 into `/opt/pqc-openssl` (first release with
   `X25519MLKEM768` and ML-DSA);
2. applies `rust/qomm-mpc/patches/expose-machine-to-embedder.patch` and
   `require-hybrid-tls.patch` from the pinned zkpi revision; the second makes
   the party refuse any TLS handshake that is not the hybrid group;
3. compiles `libSPDZ.so` and `malicious-shamir-party.x` against the 3.5.5
   headers with an rpath to `/opt/pqc-openssl/lib64`, and writes the receipt
   `.pqc-tls.sha256` over `Networking/ssl_sockets.h`, `libSPDZ.so` and
   `malicious-shamir-party.x` (the receipt `qomm-mpc`'s engine policy binds
   the engine to);
4. issues seven self-signed ML-DSA-65 certificates `P0` .. `P6` under
   `Player-Data/` with the 3.5.5 `openssl`.

The `mpc-runtime` stage re-verifies the receipt, checks with `ldd` that the
party binary resolves `libssl.so.3` from `/opt/pqc-openssl` even when
`LD_LIBRARY_PATH` names only the checkout (the MPC node launches the party that
way), and runs the node with `LD_LIBRARY_PATH=/opt/pqc-openssl/lib64:/opt/MP-SPDZ`.

The Rust binaries are linked against the same OpenSSL (`OPENSSL_DIR`,
`OPENSSL_NO_VENDOR=1`) so the gateway, participants and parties share one TLS
library; `app-runtime` ships `/opt/pqc-openssl` and fails the build if any
binary has an unresolved shared library.

The seven certificates are development material for the single-host demo. A
real deployment provisions one key per operator through its own HSM or
secret manager; QOMM never falls back to MP-SPDZ's plaintext (`-u`) mode.

## Build

Requirements on the build host: Docker Engine with BuildKit (`docker compose`
v2), network access to `github.com` and the OpenSSL release archive, roughly
20 GB of free disk for the intermediate layers, and patience: the first build
compiles OpenSSL, MP-SPDZ, the qomm workspace and the defmi workspace
(expect 30 to 60 minutes on a 16-core host; cache mounts make later builds
incremental).

```sh
git clone https://github.com/zkFMI/qomm.git
cd qomm/demo-network
docker compose build
```

`docker compose build` builds three images from the one Dockerfile:
`qomm-demo-app:local` (target `app-runtime`), `qomm-demo-mpc:local`
(`mpc-runtime`) and `qomm-demo-defmi:local` (`defmi-runtime`). Several
services declare the same image and build target; the repeated builds are
BuildKit cache hits. `docker compose build frontend` alone rebuilds
`qomm-demo-app:local` after a UI change.

Build arguments can be overridden on the command line, for example
`docker compose build --build-arg BUILD_JOBS=8`. The context is the repository
root (`context: ..`); `.dockerignore` at the root keeps `rust/target`,
`artifacts`, documents and the UI tests out of it. Long builds on a remote
host are best run inside `tmux`, with the log redirected to a file.

## Run

```sh
cd qomm/demo-network
docker compose up -d
docker compose ps           # every service should reach "healthy"
```

Start-up order is enforced by health checks: `defmi-network` first (its
`/health` waits for the five validators and the VM), then the seven MPC nodes
and the participant modules, then the gateway (which registers the twelve
entities, the 3-of-7 committee and the Maker/Taker bindings on DeFMI), then
the frontend. On a fresh set of volumes the whole network takes a few minutes
to become healthy.

- UI: `http://127.0.0.1:8800/` (English: `http://127.0.0.1:8800/?lang=en`)
- DeFMI state: `http://127.0.0.1:9650/manifest`
- Logs: `docker compose logs -f gateway`

To publish the demo on another interface, change the `ports:` entry of
`frontend` (and of `defmi-network` if the ledger view should be reachable),
or put a reverse proxy in front of `8800`; nothing else needs a host port.

Stop with `docker compose down`. Do not use `down -v` unless the demo state
is meant to be discarded: the named volumes hold the corporate keys, the
resident Maker shares of the seven nodes and the DeFMI chain. When the
running demo must keep its state and only the UI changes, rebuild the app
image and recreate only the frontend:

```sh
docker compose build frontend
docker compose up -d --no-deps --no-build frontend
```

## Acceptance without a browser

`live_acceptance.sh` drives a running project through the outage, kill,
node-abort, concurrent-request and full-restart scenarios described in
`../DEMO.md`. Every observation is taken by `qomm-live-acceptance`
(`rust/qomm-demo/src/bin/qomm_live_acceptance.rs`, shipped in the app image)
from inside the compose network over the gateway WebSocket, the Taker module
HTTP API, the MPC nodes' `/v1/maker-state` and `/v1/rounds`, and the DeFMI
JSON-RPC; the script itself only stops, kills, pauses and starts containers
and records when it did so.

```sh
cd qomm
QOMM_PROJECT=qomm-demo-network QOMM_ACCEPTANCE_OUT="$PWD/live-acceptance-out" \
  demo-network/live_acceptance.sh all
docker run --rm -v "$PWD/live-acceptance-out:/out" qomm-demo-app:local \
  qomm-live-acceptance judge --scenario outage-queue --dir /out
docker run --rm -v "$PWD/live-acceptance-out:/out" qomm-demo-app:local \
  qomm-live-acceptance report --dir /out --out /out/acceptance.json \
  --require outage-queue
```

`judge` recomputes one scenario's verdict from the recorded files; `report`
aggregates several (`--scenario` and `--require` may be repeated) and exits 1
if a required scenario is missing or failed. The script pins the Maker seats
to manual for the duration (`qomm-live-acceptance force`): a bot that rewrites
its policy every round needs a new standing pool, which needs the seven-node
FROST committee, and an outage test must stall on the corporate queue and
nothing else.

## Trust boundary

Container isolation shows the service boundaries and what crosses them; it
does not stand in for seven independent operators on seven hosts. In
production each MPC node runs under a different legal entity, OS image and
key-management system. Even in this Docker form, one MPC service only ever
receives its own share of an input, and there is no path over which parties
exchange plaintext inputs.

Development corporate keys are generated inside each named volume on first
start; the images and this compose file contain no private key or passphrase.
The gateway obtains public keys and forwards only registrations signed by the
corporate containers to DeFMI; production replaces the same signing boundary
with an HSM/KMS.

Each MPC node requires `QOMM_RECIPIENT_PARTICIPANTS`, the operator-selected
corporate service endpoints (four Makers and one Taker here). On first start
it independently reads their public snapshots, binds each claim handle to
that participant's hybrid opening key, and persists the public directory in
`recipient-opening-directory.json` under its state root. Subsequent starts
reject a changed or corrupted directory. Compose waits for these corporate
services before starting the MPC nodes. This initial enrollment trusts the
isolated demo network; it is not an independent enterprise enrollment service.

The participant volume also retains the first complete hybrid signature of
each Maker policy. Retries and restarts reuse that verified signature so that
randomized PQ signatures cannot silently change standing-pool identities.
Retain both corporate and MPC volumes together. Older proof-party state with
an empty recipient directory is not silently migrated: use a separate demo
project and retain the old state, or perform an explicitly reviewed migration.

## UI source

`qomm_demo/static/` is served as committed: `index.html`, `demo.js`,
`demo.css` and the prebuilt React Flow bundle `react-flow.js` /
`react-flow.css` (the `?v=` query in `index.html` names the UI build). The
Rust server embeds them at compile time (`rust/qomm-demo/src/web.rs`), so a
UI change needs `docker compose build frontend`. `node --test
qomm_demo/tests/*.test.mjs` checks the page against fixtures recorded from
the Rust server; `qomm_demo/DEMO_UI_JA.md` records the wording and layout
decisions of the current build.
