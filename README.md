# qomm

**QOMM** is *query-oblivious market making*. It settles through [zkpi](https://github.com/shukob/zkpi) and [defmi](https://github.com/shukob/defmi), a *zero-knowledge payment instruction* and a *decentralized financial market infrastructure*.

Oblivious market making: quote without disclosing the request, the pricing rule, or the market.

## Deployment target

QOMM is chain-independent until settlement. The product path emits zkPI instructions to DeFMI, whose current execution target is the dedicated non-EVM Avalanche L1 in the [defmi repository](https://github.com/shukob/defmi).


## What it does

```mermaid
flowchart TB
    U["a user's request<br/>asset, size, side"]

    subgraph transport["fixed cadence, additive shares, relay hops"]
        REL["every user sends the same bytes<br/>every slot, whether or not they asked"]
    end

    subgraph mpc["seven computing nodes, malicious Shamir"]
        direction TB
        PRICE["price every maker at once<br/>this is width, and it is free"]
        GATE["eligibility<br/>one comparison layer"]
        TOUR["binary tournament<br/>log2(M) comparison layers"]
        PRICE --> GATE --> TOUR
    end

    OUT["one opened key:<br/>the winning price and the winner"]

    U --> REL --> PRICE
    TOUR --> OUT

    NOTE["the trace is the same<br/>whether or not anyone asked"]
    REL -.- NOTE
    mpc -.- NOTE

    classDef cheap fill:#E8EFE6,stroke:#8FA88A,color:#243024
    classDef dear fill:#F3E4E3,stroke:#B08C89,color:#3A2A29
    class PRICE cheap
    class GATE,TOUR dear
```

## What it is made of

```mermaid
flowchart TB
    RULE["a maker's pricing rule<br/>a small expression language"]

    subgraph checker["what the checker derives, with no proof"]
        BITS["the bit width<br/>the circuit needs"]
        RANGE["the output range<br/>is bounded"]
        PLAN["the audit obligations"]
    end

    subgraph circuit["what runs"]
        GEN["generate the .mpc program"]
        COMP["MP-SPDZ compiler<br/>rounds a property of the circuit"]
        ENG["MP-SPDZ engine<br/>rounds a property of the protocol"]
        GEN --> COMP --> ENG
    end

    subgraph read["how it is measured"]
        SHIM["qomm-mpc links the engine<br/>and reads its own counters"]
        CHAN["rounds broken out<br/>by communication channel"]
        SHIM --> CHAN
    end

    RULE --> BITS --> GEN
    RULE --> RANGE
    RULE --> PLAN
    ENG --> SHIM
```

Generated from one shared research tree, which is why the layout is regular
across the three repositories. This repository is nevertheless self-contained:
its tests, locks, measurements and source do not require the private working
tree.

## What is here

Rust crates:

- `rust/qomm-dsl`
- `rust/qomm-law`
- `rust/qomm-proofs`
- `rust/qomm-sim`
- `rust/qomm-mpc`
- `rust/qomm-transport`
- `rust/qomm-audit`
- `rust/qomm-demo`
- `rust/qomm-zk`
- `rust/qomm-zkpi`
- `rust/qomm-measure`
- `rust/qomm-defmi`
- `rust/zkpi-defmi-sdk`
- `rust/qomm-harness`

Measurement binaries carried by `qomm-harness`:

- `build_audit_doc`
- `collect_origins`
- `collect_uniswapx`
- `derive_snr`
- `make_figures`
- `manifest`
- `opt_sweep`
- `report`
- `research_run`
- `run_audit_slots`
- `run_binding_chain`
- `run_bitdec_rounds`
- `run_block_range_query`
- `run_circuit_bound_proof`
- `run_disclosure_ceiling`
- `run_distributed_assembly`
- `run_distributed_dp`
- `run_distributed_publication`
- `run_dp_audit`
- `run_dp_effect`
- `run_entity_behavior`
- `run_identity`
- `run_input_check`
- `run_locate`
- `run_maker_updates`
- `run_market_thickness`
- `run_multi_asset`
- `run_multiplication_cost`
- `run_placement`
- `run_probe_budget`
- `run_price_conditionals`
- `run_qomm`
- `run_rho_sweep`
- `run_robust_atlas`
- `run_rounds`
- `run_serve_bench`
- `run_sim_matrix`
- `run_sites`
- `run_stages`
- `run_staleness`
- `run_three_times`
- `run_threshold_assembly`
- `run_transport`
- `rust_only_guard`
- `scrub_artifacts`
- `serve_demo`
- `sweep`
- `tcp_rtt`

`artifacts/` holds the measurements the numbers in the paper are taken from, as
the binaries wrote them. Each carries the host it ran on as a label (`host-a`,
`host-b`, `host-c`) rather than a machine name; the private mapping back
to real machines is not published.

## Documents

- [`AUDIT.md`](AUDIT.md) --- what the audit machinery checks, what it catches, and what it costs
- [`DEPLOYMENT.md`](DEPLOYMENT.md) --- what every measurement implies for what to deploy where
- [`BINDING.md`](BINDING.md) --- the one gap between what was computed and what was committed, the two ways to close it, and what each one costs
- [`REGULATION.md`](REGULATION.md) --- which accounts and which statutes a live deployment touches, in Japan and in four other jurisdictions
- [`POSITION.md`](POSITION.md) --- what is new here and what is not, stated line by line against the nearest prior work
- [`ACCOUNTABILITY.md`](ACCOUNTABILITY.md) --- what happens when a node misbehaves: the five rungs from abort to guaranteed output delivery, and which one each mechanism here reaches
- [`DEMO.md`](DEMO.md) --- a demonstration a room can operate one seat each, and what is real in it
- [`REVIEW.md`](REVIEW.md) --- what two rounds of review found, including what was checked and found sound
- [`ZKPI_WIRE.md`](ZKPI_WIRE.md) --- the bytes an instruction travels as, the vectors to check an implementation against, and where it can run

## Running it

```sh
cd rust
cargo test -j 4 --locked --workspace
```

## Measurements

Every reported number has an artifact and a Rust binary that produces it. Where a
measurement needs something not shipped here --- MP-SPDZ, a second host, a market
data feed --- the binary says so and fails rather than substituting a default.

## License

MIT. See `LICENSE`.
