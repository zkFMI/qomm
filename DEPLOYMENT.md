# Choosing a deployment

What every measurement implies for what to pick under which operating
conditions. The numbers come from `RESULTS.md` (through stage 3),
`OPTIMIZATION.md` (speed), `AUDIT.md` (audit, transport, proofs) and
`SURVEY.md` (comparison of schemes). **Estimates and measurements are marked
apart.**

---

## 0. The first decision: where the nodes sit

This matters more than any other choice. Measured (M=16, 31 bits, malicious
Shamir, N=7, T=2):

| node placement | one-way delay | one quote | RFS (k=5) update rate |
|---|---:|---:|---:|
| same rack | 0 ms | 0.166 s | 12.0/s |
| same metro | 1 ms | 0.617 s | 3.0/s |
| domestic wide area | 5 ms | 1.619 s | 1.2/s |
| Tokyo to Singapore | 15 ms | 3.876 s | 0.48/s |

Of the 3.68 s at 15 ms one way, **2.13 s is pure round-trip time** that no
circuit change touches. **Putting the seven nodes in one metro is six times
more effective** than any stack of cryptographic improvements.

Against that, putting them in one metro **raises the correlation of collusion**.
Seven nodes in the same jurisdiction and the same commercial orbit leave the
T=2 assumption resting on organisational independence alone, not geography.
That is governance, not technology, and it is an explicit trade against latency.

### 0.1 Independence cannot be bought inside a committee

The obvious compromise is six nodes in one metro and a seventh somewhere with a
different legal order --- geographic independence for the price of one node.
Measured at 120 ms one way, which is Tokyo to Zurich or Tokyo to London:

| node placement | one quote | vs all near |
|---|---:|---:|
| all near (1 ms) | 0.517 s | --- |
| **one far (six at 1 ms, one at 120 ms)** | **22.998 ± 0.733 s (n=3)** | **44x** |
| one near (six at 120 ms, one at 1 ms) | 25.682 ± 1.090 s | 50x |
| all far (120 ms) | 26.148 ± 1.209 s | 51x |

**One distant node costs 86% of what moving all seven costs**, against 82% at
15 ms (`placement.json`): the penalty for a single outlier grows with distance
rather than shrinking. The compromise is not a compromise. Every placement here
was verified against the plaintext answer.

The round count is what does this. It is 70, flat, independent of the number of
makers and of the number of assets, so the wall clock carries 70 x RTT of pure
waiting on whichever link is slowest.

*Prediction and miss, recorded.* 70 x 0.240 s = 16.8 s of round-trip plus about
1.5 s of computation predicted **~18 s**, from a model validated at 15 ms to
1.4% (predicted 2.10 s of RTT against the 2.13 s above). Measured **23.0 s**.
The linear model **under-predicts by 28%** at eight times the distance it was
checked at, so extrapolating it further should not be trusted either.

### 0.2 What 26 s is worth, which is not the same as how long it is

The table above says a globally spread committee is fifty times slower. It does
not say that matters, and the number that decides it is not the delay but
**whether the price moved during it.**

UniswapX fills carry a rate that really was executable at a block, so the drift
across a gap can be set against the dispersion the same market shows *within*
one block --- spread, size impact, fee tier, all the ways the price is not a
single number even at an instant. Ethereum blocks are about 12 s, so one
cross-region quote is two of them (`staleness.json`, four busiest pairs):

| gap | drift | vs the within-block floor |
|---|---:|---:|
| within one block | 4.8--7.5 bp | --- |
| **2 blocks, ~26 s: one cross-region quote** | 3.6--8.6 bp | **1.01x** (median) |
| 8 blocks, ~96 s | 7.4--9.4 bp | 1.34--1.75x |
| 25 blocks, ~300 s | 12.2--19.2 bp | 1.63--4.00x |

**Twenty-six seconds of drift is the same size as the price uncertainty the
market already has inside a single block.** A quote that took 26 s to compute is
not distinguishable from an instantaneous one, because the price was never that
precisely defined. And this is crypto on Ethereum L1 --- the harshest case
available. An instrument that moves less makes a slow quote cheaper, never
dearer, so this bounds the bond and derivative cases from above rather than
describing them.

Caveats, because the measurement has them: these are executed rates, so a filler
chose the moment; most blocks carry one fill, so the within-block series is 60
to 247 observations per pair; and the floor mixes spread with size impact rather
than being a quoted bid-ask.

**So the criterion is `drift(latency)` against the instrument's own price
uncertainty, and not the latency.** Where the spread is basis points, a
cross-region committee is affordable. Where it is a fraction of one, it is not.

### 0.3 What follows: the tier is per instrument

- **A committee spanning regions is available for request-for-quote**, at 26 s
  and with the geographic independence that a metro deployment cannot have.
  Where leakage hurts most --- wide spread, thin book, large size --- it is also
  where 26 s is cheapest. **The economics and the physics point the same way.**
- **Request-for-stream survives it better than request-for-quote does**, which
  is the opposite of what it looks like. A taker on a 26 s stream acts whenever
  they like, on a price 13 s old on average and 26 s old at worst, and waits for
  nothing. A taker on a 26 s request-for-quote waits 26 s and then acts on a
  price that is 26 s old. **Same drift, and the stream removes the round trip
  from the critical path.** The refresh rate falls; the mode does not fail.
  What a slow stream really costs is on the maker's side --- a quote up to 26 s
  old can be taken by someone watching a faster feed, so the maker widens by
  about the drift, which is the same 4.8--8.6 bp. On a five-basis-point
  instrument that roughly doubles the spread; on a fifty-basis-point one it
  disappears.
- **Crossing regions to settle was never round-bound anyway.** Two ledgers
  sharing no state settle through an adaptor signature and a deadline, costing
  0.06 ms of cryptography against an exposure set by the two ledgers' finality
  (`DEFMI.md` section 6).
- **A maker quoting outside its own region needs no committee spanning both.**
  Policy registration and quote computation are already separate: the rule
  crosses once, offline, where latency does not matter.

### 0.4 What a maker can decline

Every term in the price rule is a choice the maker makes, and all but one of
them were already declinable by setting a coefficient to zero: `slope` switches
off the size term, `invcoef` the inventory term, `active` the maker. The
reference price was the exception --- it was added with a hard-wired coefficient
of one, so a maker in a market with no usable benchmark had no way to say no,
and a corporate bond has no continuous mid to be an offset from.

`use_ref` is that missing switch: a secret bit per maker, with
`anchored = ask_level + use_ref * ref`. It puts one more multiplication into a
SIMD layer that already has two, so it does not add multiplicative depth. The
current Rust generator and its reference-invariance tests exercise both the
anchored and independent-price modes; no retired cross-language timing is kept
as present evidence.

The two settings are two markets, and `qomm_dsl/examples/` now carries one file
for each. On the reference, `mid` is a small offset and the market level rides
along --- which is what section 0.5 is about. Off it, `mid` carries the level
itself, so it has to be declared wide enough to hold one, and a wider parameter
is a wider range proof: 12 bits round up to a 16-bit proof, 18 bits to a 32-bit
one.

**There is a cheaper version of the same switch, and it is worth knowing which
one is being bought.** Whether a market has a usable benchmark is a fact about
the market, not about the maker, and putting the switch there costs nothing at
all: a market with no benchmark is a **zero row in the public reference table**,
folded into a lookup that already happens. No field, no multiplication, no
bytes. What it gives up is that the venue decides rather than the maker.

The maker-side switch buys that choice, and pays for it twice: 0.26% of traffic,
and **the correction of section 0.5, whenever makers on one asset disagree.**
The correction works because the reference shifts every maker's cost equally and
so cannot reorder them --- which stops being true the moment some makers are on
the reference and others are not. A relative maker at `ref + 30` and an absolute
one at `100,020` swap places as the reference crosses `99,990`;
`rust/qomm-mpc/tests/reference_invariance.rs` holds the case.

So: **per-market is free and keeps the correction, per-maker costs 0.26% and
keeps the correction only while an asset's makers agree.** A venue that wants
both can take the maker-declared flag and require it to be consistent per asset
at registration, which is a rule about admission rather than about the circuit.

### 0.5 The drift need not be paid at all

The quote is affine in the reference price and the winner does not depend on it.
`anchored = ask_level + spread_request(ref)` adds the same term to every maker, and no
eligibility gate --- asset, size, expiry, active --- reads the reference, so a
move in it shifts every cost by the same amount and cannot reorder them.
`rust/qomm-mpc/tests/reference_invariance.rs` checks this rather than asserting it.

So a slow committee does not have to produce a stale quote. **Run against
whatever reference was current when the computation started, and correct the
revealed price by how far the reference moved in the meantime.** The winner
needs no correction; it was never wrong. The correction costs no rounds: it is
a secret one-hot vector times public constants, which is local, and with one
asset it is a public addition the taker can do unaided.

**This is a property the design has and the implementation does not yet use.**
`REF_TABLE` is fixed when the circuit is parameterised and an absolute price is
revealed, so today's output really is as old as the computation. Making it
current is a change to what is revealed, not to the circuit's cost.

Two things stay genuinely stale and are worth stating. The inventory term moves
when a maker trades rather than continuously, so it ages far more slowly than
the mid. And `expiry` is compared against the `now_t` the run started with, so a
policy expiring inside the window can be admitted wrongly --- evaluating expiry
against `now_t` plus the expected latency is the fix, and it costs nothing.

`REGULATION.md` section 5 takes up what each choice costs in trust, since a
regional committee and a spread one do not defend against the same adversary.

---

## 0.6 Binding what was computed to what was committed

A node can check its own share against the dealer's commitment before it
computes. **What nothing forces is that the value it then puts into MP-SPDZ is
the share it checked** --- the signature on a dealt share buys detection and
attribution, not prevention.

Two things bear on the placement decision, and the rest is in `BINDING.md`.

- **It is closable in the default field for one round and six kilobytes**
  (`input_check.json`): 65 rounds against 64, 19.38 MB against 19.37, 47 ms.
  Nothing about the node placement changes.
- **Making the quote proof publicly verifiable costs six to eight per cent.**
  That needs the MPC to run over the commitment group's order, and measured with
  the field width compiled in and the prime given at run time, it is **1.00x the
  rounds, 2.00x the traffic and 1.07x the wall clock** --- 1.06x cross-region,
  where the unchanged round count is what matters (`matched_field.json`).
  **Nothing about node placement changes.** An earlier version of this section
  reported 2.14x and 14.3x, from compiling the prime in rather than passing it at
  run time; `BINDING.md` section 2.1 has that story.

`BINDING.md` has the two mechanisms, what each costs, why no other group helps,
what post-quantum costs, what a quantum network would and would not change, and
every prediction in that work that missed.

## 1. Three deployment profiles

The measurements narrow the realistic choices to three.

### Profile A: metro, no audit, latency first

| item | setting | why |
|---|---|---|
| node placement | same metro (1 ms one way) | 0.617 s per quote |
| bit length | 31 | 42% less traffic, enough for prices around 100,000 ticks |
| tournament | binary | raising the arity does not change the wall clock |
| batch Q | matched to the arrival rate (Q=3 for a one-second slot) | rounds belong to the job |
| proofs | **no** quote proof | avoids +550 ms to complete the proof |
| disclosure | threshold disclosure (arm B) | DP disclosure measured significantly worse |
| protocol | malicious Shamir | 1.12 to 1.24x the wall clock over semi-honest. Cheap |
| binding | **Shamir inputs over the group order, and the per-party input check** | see 1.2. Not on by default, and the quote proof means little without it |

**What it gives**: sub-second quotes, secrecy for requests that do not settle
(AUC 0.500).
**What it does not**: a proof that the computation was right. Receipts bind a
node to a result; they do not show the result was correct.

### Profile B: metro, audited, near-immediate

| item | setting | why |
|---|---|---|
| node placement | same metro (1 ms one way) | |
| proofs | quote proof, M <= 16 | 317 ms to prove, 400 ms to verify |
| the three times | priced 824 ms, proved +551 ms, settleable +655 ms = **2.03 s** | measured |
| RFS update interval | set it to **3 s or more** | one second is not met (measured) |
| disclosure interval | 60 s | proofs and audits fit comfortably; also what would make 63.9 MB of in-MPC noise per release affordable, were that protocol deployed (2.3) |
| binding | **required here** | see 1.2. A quote proof about inputs nothing checked is a proof of the wrong statement |

**What it gives**: the computation is verifiable every slot. The joint path
means no node holds the witness *of the sigma opening it assembles*; the quote
proof as a whole is produced by a prover holding the witness, because its range
proofs cannot be assembled from shares --- see `AUDIT.md`.
**What it does not**: a sub-second settleable time, or a one-second RFS.

### Profile C: wide area, batched

| item | setting | why |
|---|---|---|
| node placement | geographically spread (15 ms one way) | when governance independence comes first |
| batch Q | 16 to 32 | 284 ms per quote at Q=32 (measured) |
| one user's wait | 9.09 s at Q=32 | the whole job's duration |
| suited to | markets that value secrecy and dispersion over immediacy | |
| binding | as Profile B | the traffic it costs is 2.03x, which is the profile that can afford it least and needs it as much |

**Choosing Q**: time per quote falls monotonically as `T(Q)/Q` while the wait
rises as `T(Q)`. At an arrival rate of lambda per second and a slot period of
`S` seconds, `Q ~ lambda*S` fills naturally. Measured at 15 ms one way,
`T(Q) ~ 3.4 + 0.18Q` seconds for Q <= 32, so for a target wait `W` take the
largest Q with `Q <= (W - 3.4)/0.18`.

### 1.2 What binds the computation to the commitments, and is off by default

This is the switch most easily missed, so it is stated on its own.

A quote proof proves that the opened price is the correct function *of the
inputs the nodes used*. It says nothing about whether those inputs are the
shares that were dealt and committed. Two mechanisms close that, they catch
different parties, and **both are command-line flags that default to off**:

- `--shamir-inputs` runs the circuit over the commitment group's scalar field,
  so the shares the nodes already hold serve as witness shares. This is what
  makes the proof assemble at all. It catches **a dealer** that deals what it
  did not commit --- measured, at party 3, position 8, before anything is
  computed.
- `--input-check` opens one linear combination per node after the inputs are
  fed. It catches **a node** that passed the share check and then supplied
  something else --- which the first mechanism does not, because a substituted
  input is a valid share of a different number and every commitment still
  opens.

Measured together on one market (`artifacts/binding_chain.json`, six arms, all
verified against the cleartext reference):

| inputs | field | check | rounds | global traffic |
|---|---|---|---:|---:|
| additive | default | none | 57 | 9.30 MB |
| additive | default | per-party | 59 | 9.44 MB |
| Shamir | group order | none | 57 | 18.60 MB |
| **Shamir** | **group order** | **per-party** | **59** | **18.88 MB** |

So the full arrangement costs **two rounds and 2.03x the traffic** against a run
with neither. The traffic factor is the element width going from sixteen bytes
to thirty-two and nothing else; the two rounds are the check.

**Why the defaults are off, and why that is not an argument for leaving them
off.** The flags default to off in the measurement harness because every arm has
to be selectable and a default that doubled the traffic would make the arms
incomparable. That is a property of a research harness. A deployment has no such
reason, and a deployment that ran the audited profile without these would be
publishing a proof about inputs that nothing checked --- which is a proof of a
different statement than the one the venue is claiming.

### 1.1 Preprocessing between slots, which applies to all three

The correlated randomness a round consumes does not depend on the request. It
depends on the shape of the circuit, and the shape is fixed and compiled once.
So it can be generated before the request arrives, and `AUDIT.md` measures what
that is worth: with preprocessing on disk the online phase is **16% of party
0's bytes** --- 19% of the global total, since a party's share depends on where
it sits in the reconstruction --- **and 71% of the rounds** (malicious Shamir, 7 parties, T=2, 8 makers).

Read that as bandwidth, not latency. Rounds are what a wide-area profile is
mostly made of, and 44 rounds at a 17.4 ms round trip is still 0.77 s that
nothing removes, because those round trips are circuit depth. Profile C is the
one this changes most --- it is the profile whose cost is bytes over a long
link --- and it changes Profile A least.

The caveat matters as much as the number. The measurement used a trusted
dealer to write the preprocessing, which no deployment can do; a real offline
phase runs among the nodes and costs more than the dealer did. What is
established is the size of the online phase, so the design question this
settles is *whether it is worth building an offline phase* --- at a sixth of
the bytes, yes --- and not what that phase will cost.

---

## 2. Choosing each component

### 2.1 Anonymous credentials (KYB)

| registry size | what limits it | choice | measured basis |
|---|---|---|---|
| N <~ 8 | proving, on the maker's device | **OR composition** | 1.21 ms to prove at N=8 |
| N >~ 16 | verifying, at the venue or on chain | **Groth-Kohlweiss** | 2.35x faster to verify and 5.1x smaller to prove at N=128 |
| attributes to hide too | anonymity set | BBS+ family (not built) | re-randomisable |

Today's cohort registry holds a handful to a few dozen firms, so **OR
composition is the default**. If the venue starts verifying many presentations
per slot, swap in `rust/qomm-zk/src/oneofmany.rs`; the two share the group abstraction, so
the change is local.

### 2.2 Range proofs

This implementation (bit decomposition) **does not get faster in batch** ---
2.9 ms each, flat, measured. Bulletproofs in the literature reach 0.24 ms per
range in batch verification and 74 us with aggregation. So:

| use | how many | choice |
|---|---|---|
| maker policy audit, at registration | 6 fields | **this implementation is enough** (15.9 / 15.4 ms, negligible against a 60 s disclosure interval) |
| proving a quote correct | M of them, dominant at M=16 | **replace with Bulletproofs** (not built; verification expected to fall by an order of magnitude) |

That last effect is **an estimate, not a measurement**. Building it and
measuring it is the next piece of work.

### 2.3 Verifiable DP

Of D1 to D4 in `SURVEY.md` section 4, **D1+D2 are the default**, D3 is a future
extension and D4 is not taken. The reasons are a mismatched threat model --- D4
assumes a single prover that knows the plaintext aggregate --- and a cost two
orders of magnitude apart.

**But DP disclosure itself is not recommended at present.** It measured
significantly worse than no disclosure at all (fill rate -0.039, maker P&L
-100.7, both beyond the minimum detectable difference). The cause is identified
as upward bias in a non-linear statistic, so **arm B stays the default until a
bias correction is added and the measurement repeated**.

**The publication path now generates its noise inside MPC and commits it with
the budget transition.** Seven MP-SPDZ processes supply the 64 shared random
bits; seven isolated signers certify the transcript and output; the ledger
atomically spends the entity budget and rejects replay or alteration. The
end-to-end acceptance is `artifacts/distributed_publication.json`; the detailed
cost curve remains `artifacts/distributed_dp.json`. Measured at n=7, T=2:

| | rounds | traffic (7 nodes) | per node |
|---|---|---|---|
| the release with no mechanism | 2 | 0.0022 MB | 0.3 KB |
| support 8 | 28 | 32.1 MB | 5.2 MB |
| support 16 | 28 | 63.9 MB | 10.3 MB |
| support 32 | 43 | 127.6 MB | 20.1 MB |

**The noise would be 92.9% of the rounds and above 99.9% of the bytes.** Read
that against the 60 s disclosure interval this section already recommends: 63.9
MB per minute is 8.5 Mbit/s across all seven nodes and 1.4 Mbit/s at party 0,
which sends the most of any single node, and
the 28 rounds are about 56 ms at the same-metro placement above. It fits with
three orders of magnitude to spare. At one release per second the same figures
are 511 Mbit/s aggregate and 83 Mbit/s at party 0, which a datacentre link carries
and a wide-area one may not; the 28 rounds are then 56 ms of a 1000 ms period at
the metro placement and about 2 s at a 70 ms round trip, where they no longer
fit. **The interval this file already recommends is what makes the protocol
affordable, and the recommendation does not depend on the protocol.**

The implemented mechanism carries a finite support and
folds the tails onto its endpoints, which the central sampler does not do. That
costs 1.8e-4 in total variation from the ideal at support 8 and 6.1e-8 at
support 16 --- against a rounding term of 2.4e-16 to 6.7e-16, which is set by
the f64 accumulation behind the thresholds and not by the 2^-64 grid they are
floored onto --- and worse, it costs the *form* of the guarantee: adjacent
inputs give releases whose supports do not coincide at the edges, so the ratio
there is infinite and the mechanism is **(epsilon, delta) with delta 2.5e-4 at
support 8**, computed as a hockey-stick divergence rather than from a closed
form. Widening the support does not fix it
cheaply: the 64-bit uniform cannot represent a cell below 2^-64, which at
epsilon 1 stops near |k|=35, and going past that is paid for in shared bits per
release. The production certificate therefore states `(epsilon, delta)` with
delta from `DpMechanism::privacy_delta` --- the hockey-stick divergence over
the cells actually released, 2.5e-4 at support 8 --- instead of silently
substituting it under the appendix's ideal pure-DP claim. Do not use a closed
form for it: the
folded tail understates it by e^epsilon and the endpoint cell understates it
whenever the sensitivity exceeds one, by about 2x at epsilon 0.5 with
sensitivity 3.

There is also a cheaper construction that is not implemented --- each node
drawing its own share from a Polya distribution with shape 1/(n-T), so that the
honest n-T sum to exactly the discrete Laplace, at 2 rounds and no extra traffic
for 1.4x the target variance (about 1.18x the standard deviation). It is not
taken because a corrupt node can then draw from the wrong distribution and skew
the published value; the in-MPC protocol makes that impossible. See
`POSITION.md` for why the argument against that objection is not obviously
wrong.

### 2.4 Threat model

| | rounds | traffic | wall clock |
|---|---|---|---|
| semi-honest Shamir | 82 | 3.79 MB | 3.83 s |
| malicious Shamir | 100 | 12.16 MB | 4.73 s |

**Malicious security costs 1.12 to 1.24x the wall clock and 2.9 to 3.2x the
traffic.** At that price, requiring it is a reasonable call and there is no
reason to drop it.

**That price is a property of the circuit, not of the engine, and the disclosure
circuit is dearer.** The same comparison on the noise generation of 2.3, at
support 16: semi-honest 15 rounds and 10.6 MB, malicious 28 rounds and 63.9 MB
--- **1.9x the rounds and 6.0x the traffic**, against 1.2x and 3.2x on the quote
circuit. The quote circuit is arithmetic and the disclosure circuit is
comparisons, whose malicious-secure random bits are verified inline rather than
sacrificed in a batch at the end. A prediction that the ratio would carry over
from one circuit to the other was written down before this was measured and was
falsified; it is recorded in `REVIEW.md`.

The conclusion does not change. Even semi-honest, the in-MPC release costs 15
rounds against 3 for the same release with no mechanism, so weakening the threat
model does not make this construction cheap --- it only makes it less expensive.
Keep malicious security.

### 2.5 Multiple assets

**The round count does not depend on the number of assets** --- 70 rounds flat
from 1 to 32 assets, measured. Only traffic grows, by about 0.04 MB per asset.
**There is no reason to split the job per market.**

Settling in the clear reveals the market from the asset that moves, so hiding
the market only means something together with the settlement side. zkPI
(section 2.6) is what fills that.

### 2.6 zkPI: closing the settlement side

Making the payment instruction itself a commitment and a proof lets the
settlement venue confirm only that it is a valid, unspent instruction that
matches the rules, without reading the asset, the quantity, the price or the
counterparties. Measured on Ed25519:

| | measured |
|---|---:|
| issuing an instruction (range proofs plus quorum signature) | 11.8 ms |
| verification at the venue | 12.6 ms |
| nullifier | 32 B, hides the legal entity |

**This carries the market selection hidden inside the MPC all the way through
settlement.** The venue needs only `verify` and the nullifier, so **it drops
into another DEX unchanged**: it never needs to know how the price was reached.

Implication: if multiple assets are in scope, take zkPI with them. Combined with
plaintext settlement, the secrecy paid for in the circuit is lost at settlement.

### 2.7 The width of a settlement rail

DeFMI's settlement cost is set almost entirely by **the ledger's balance
width**, not by the choice of cryptography. Measured (`DEFMI.md`): 0.44 ms/bit
to build, 0.55 ms/bit to verify, 448 B/bit on the wire. At 40 bits, 26.1 ms of
the verification is zkPI itself and does not depend on the width.

| decision | choice | effect |
|---|---|---|
| securities rail width | from the largest quantity to be listed; 24 bits is 16 million units | 48 to 32 bits cuts verification 12% and 5,376 B |
| cash rail width | the settlement currency's smallest unit times the expected ceiling; 40 bits is 1.1 trillion | as above |
| same width for both? | **no** | securities and cash are orders of magnitude apart |

This is the only lever that works without changing a line of cryptography. It is
a listing decision, not a technical one.

**The Rust implementation blunts this lever.** Bulletproofs comes only in powers
of two, so securities at 24 bits round up to 32 and cash at 40 to 64 (section
2.12). The retired arbitrary-width prototype allowed finer trimming; the Rust
implementation deliberately exposes the Bulletproofs widths only.

### 2.8 Hiding the instrument at the settlement layer

The MPC layer hides which market a request is for (`multi_asset.json`: 70 rounds
flat from 1 to 32 assets). Dropping onto a per-instrument rail at settlement
gives that back, so hiding it means using an asset tag.

| choice | when | cost |
|---|---|---|
| per-instrument rails | a venue where the instrument is public (listed equities) | zero, but the instrument leaks to the settlement layer |
| asset tags | where the instrument is itself information (block trades, private placements) | +32 B and +1.7% to build per settlement at 64 instruments; 20.3 ms / 1,344 B for the membership proof at issue |

The tag's soundness rests on the range proof on the difference. Neither a
registered tag for a different instrument nor a point that was never registered
can move a balance (both measured as `rejected`). The membership proof is needed
**only when a balance is issued**.

### 2.9 How far to take netting

| decision | choice | effect |
|---|---|---|
| gross or net, per rail | gross if failure is not tolerable, net to get the liquidity | gross is order dependent and refuses; net can fail at the close |
| instruction granularity | one zkPI per trade, or one attestation per cycle | per trade takes 12.6 s to verify at N=256; per cycle takes 0.2 s (**67.7x**) |
| how far net may go | the risk the FMI takes on, capped by collateral after haircut | the coverage proof costs the same either way (9.2 to 9.2 ms), and the sign is hidden too |
| waterfall tranches | four, as in practice | about 9.1 ms per tranche, run once per default |

**Checking at the order and checking at the close are a trade.** A gross rail
removes settlement failure by construction, but a participant receiving 100 and
delivering 100 is refused when the delivery arrives first --- which is exactly
the liquidity saving netting exists for. Practice answers with a limit; the Bank
of Japan's intraday overdraft is that answer.

Collapsing the instruction to one per cycle makes the settlement layer's work
independent of the number of trades, but individual trades are then no longer
verified. Conservation and coverage remain, and each participant can check its
own net, so what is lost is **third-party verifiability of the allocation**.
That is what a central counterparty has always been; this only makes it
explicit.

### 2.10 Hiding the counterparties (the note ledger)

An asset tag hides only *what*. With fixed handles, *who with whom* remains. A
note ledger buys an anonymity set sized by the ring, but the cost is
**asymmetric**.

| ring size | prove (payer) | verify (node) | wire |
| ---: | ---: | ---: | ---: |
| 8 | 19.5 ms | 22.8 ms | 19,232 B |
| 64 | 32.1 ms | 27.2 ms | 19,904 B |
| 512 | 159.3 ms | 56.3 ms | 20,576 B |

A settlement node handles a ring of 512 in 56.3 ms. What binds is the payer's
159.3 ms. So the ring size is set by **the payer's device**, and 64 (32.1 ms to
prove) is the sensible mark.

Putting a whole DvP on note rails, against 68.3 ms for the account version:
68.8 ms at ring 2 (+1%), 72.8 ms at ring 16 (+7%), 80.8 ms at ring 64 (+18%).

| decision | choice | effect |
|---|---|---|
| accounts or notes | notes when the counterparty relation is itself information | +1% to verify at ring 2, +18% at ring 64; counterparties narrow to the ring |
| ring size | the largest the payer's device tolerates | the wire grows only 224 B per doubling, so bandwidth does not decide it |
| pool size | the scan cost falls on the payee | 0.052 ms each; 5.2 s per scan over 100,000 |

Only the sender can tell that a note they made was spent, because they know the
`g^S`. That is unavoidable in this construction, so hiding it from the sender
needs one intermediate move.

### 2.11 Relays

| hops | slot wall clock | origin-linking AUC |
|---|---:|---:|
| 1 | 34.7 ms | 0.500 |
| 2 | 38.3 ms | 0.500 |
| 3 | 43.5 ms | 0.500 |

**About 4.4 ms per hop.** Every hop on the path must be compromised to follow a
message, and that gets harder with each one. A single hop lets the relay see the
user's IP, so **two or more is recommended**. The cost is negligible.

### 2.12 What the settlement layer costs, and what it does not carry

Measured on the same machine (`DEFMI.md` section 7). Bulletproofs widths are
powers of two, so a 40-bit rail is proved at 64.

| | |
|---|---:|
| settle (verify) | **7.79 ms** |
| package | **2,816 B** |
| per core | **128/s** |
| rail width granularity | 8/16/32/64 only |
| audit of the underlying cryptography | dalek and bulletproofs (Quarkslab 2019), FROST (NCC 2023) |

Two limits to plan around. Rail widths round up, so a 24-bit securities rail is
proved at 32 and a 40-bit cash rail at 64 --- the split-rail lever is blunter
than the widths suggest. Note-rail DvP is implemented: pre-trade reserve notes
are consumed atomically into delivery and refund claims, and the full
Avalanche acceptance completes with zero account records. What remains external
is legal recognition of those claims and the production operator network.

---

## 3. What has to be decided before deployment, and is not decided by technology

1. **Independence of the node operators.** T=2 tolerates two colluding nodes.
   Placing them in one metro gives up geographic independence, so operator,
   jurisdictional and corporate-parent independence has to carry it instead.
2. **The size of the bond.** The slashing that is implemented carries only
   relative weights --- 1,000,000 for a double signature, 500,000 for an
   omission, 250,000 for stale state, 50,000 for a missing receipt. The absolute
   figure follows from an estimate of what misbehaving could earn.
3. **The target RFS update interval.** With auditing, set it to 3 s or more.
   Requiring one second means choosing between a lighter proof (replacing the
   range proofs with Bulletproofs) and dropping the audit.
4. **A per-entity cap, and where its counter lives.** Reading a maker's
   inventory off its own two-sided quotes is the one attack the cryptography
   does not touch, because a firm price is what the protocol exists to return.
   Measured (`artifacts/probe_budget.json`): the correlation is about 0.53 and
   **does not grow with the budget** --- what grows is the confidence, from a
   correlation distinguishable from zero in 23% of seeds at ten probes, to a
   majority at **24**, to essentially all at **96**. So the cap sets how often
   an entity can refresh its picture, not whether it gets one. The shipped
   default is 60 requests an epoch, which sits between those two figures.

   Two things the cap needs that the code does not give it. `EntityRateLimiter`
   holds its counters in memory, so **they reset when the venue restarts and
   they do not compose across instances** --- a venue run as two processes for
   capacity gives every entity twice the allowance. Persisting them, and
   sharing them, is deployment work that has to happen before the cap means
   what this section says it means.
5. **What the settlement venue will accept.** zkPI's `InstructionBounds` --- the
   floor and ceiling on quantity and price, and the deadline horizon --- are the
   venue's to publish. Narrow is safer and also refuses legitimate trades. That
   is venue policy, not technology.

---

## 4. Unverified, and still bearing on the deployment decision

| item | state | bearing |
|---|---|---|
| a real seven-site deployment | measured to two sites (real RTT 17.4 ms, 1.66 s net) | operating and recovering seven sites is unknown. **The largest remaining unknown**, now that the offline split is measured |
| disclosure halt rate in a thin market | unverified | if arm B always withholds in a thin market, the choice of disclosure changes |
| reproduction on other data | unverified | the stage-3 conclusions stand on generated data |
| the effect of replacing with Bulletproofs | **measured** (Rust, `artifacts/rust_bench.json`) | 10.5x on the package, 6.3x on verification, at the price of widths fixed to powers of two |
| the fixed cost of a quote | **measured** (`OPTIMIZATION.md` section 3.7) | 406 to 31 ms per quote at 0 ms one way, but the split is 9.8x from batching and 2.0x from residency, so **a resident service is not needed outside a low-latency deployment** |
| keeping the MPC party processes resident | not built (needs TLS certificates) | the fixed cost that remains is 5.3 ms per quote at Q=32; it only matters at Q=1 |
