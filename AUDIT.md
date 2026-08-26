# An MPC audit that does not leak the request

The property at the centre of this stage is that **neither whether a request was made nor which market it was for may be told apart from outside, while a computing node's misbehaviour remains detectable**.
This document is generated from the measurement JSON by `make audit-doc`. No number in it was typed by hand.

Raw data: `artifacts/audit_slots.json`, `transport.json`, `quote_proof.json`, `three_times.json`, `multi_asset.json`, `rounds.json`, `stages.json`, `rounds_by_channel.json`.

---

## 1. What a cover slot and a real slot leave behind

A secret `is_real` bit goes into the circuit and is never branched on. The shape of the circuit does not change; the bit decides only whether the market makers' state moves.

| | rounds | sent per party | wall clock, median |
|---|---:|---:|---:|
| real slot | 287 | 17.9016 MB | 1.3187 s |
| cover slot | 287 | 17.9016 MB | 1.3196 s |

Compiler statistics, runtime round count and bytes sent all agree (True). The wall-clock gap of 0.0008 s is smaller than the 0.0015 s spread seen when the same condition is repeated.

**Boundary**: this is the trace of the MPC job. The path from a user to the nodes is measured separately in section 3.

---

## 2. Three kinds of audit

Correctness of the computation is handled by a per-slot digest of the result, contradictory signatures by two receipts, and failure to answer by a deadline and an absence. Without a rule that **every node emits a receipt every slot whether or not there was a real request**, 'it did not answer' cannot be shown to a third party.

Dropping an eligible market maker is caught by fixing the set to a single digest in advance. No hash needs to be computed inside the circuit.

### Fault injection (7 nodes, 6 slots, quorum 5)

| fault injected | node | slot |
|---|---:|---:|
| equivocation | 2 | 1 |
| omitted_makers | 3 | 2 |
| stale_state | 4 | 3 |
| missing_receipt | 5 | 4 |

- all detected: **True**, missed **0**
- wrongly accused honest nodes: **0**
- 1 consequential findings: a node that double-signs also signs a state the quorum does not take, so it is guilty twice. That is not a false positive.

| node | slot | fault | slashed | bond left |
|---:|---:|---|---:|---:|
| 2 | 1 | equivocation | 1,000,000 | 1,000,000 |
| 2 | 1 | forked_state | 500,000 | 500,000 |
| 3 | 2 | omitted_makers | 500,000 | 1,500,000 |
| 4 | 3 | stale_state | 250,000 | 1,750,000 |
| 5 | 4 | missing_receipt | 50,000 | 1,950,000 |

The penalties differ by the nature of the fault. A double signature is self-contained evidence that admits no excuse, so it is heaviest; a missing receipt happens to honest nodes too, so it is lightest.

---

## 3. From user to node: a fixed cadence and multiple relay hops

- **Fixed cadence**: one message of the same length to each node every slot, whether or not there is an order. In a design that speaks only when it needs to, speaking is itself the announcement.
- **Additive secret sharing**: split into one share per node before leaving the device. A single share alone is uniform noise.
- **Relay hops**: each hop holds until the slot boundary and reshuffles. The first hop knows the sender's address; the second knows only the first.

| hops | sent per user per slot | origin-linking AUC | one relay can recover an order | slot wall clock |
|---|---:|---:|---|---:|
| 1 | 2121 B | 0.500 | False | 36.2 ms |
| 2 | 2121 B | 0.500 | False | 76.0 ms |
| 3 | 2121 B | 0.500 | False | 108.4 ms |

A user who sent an order (2121 B) and one who did not (2121 B) send the same amount. The batch a node sees is [12] regardless of how active anyone was.

**Boundary**: hops after the first are implemented as in-process hand-offs, so a real deployment adds one network round trip per hop and the wall-clock figures here do not include that. If a relay colludes with its own node, that one share is linkable; recovering the order needs every node to collude.

---

## 4. Proving the computation itself was right

A sigma protocol's response is **linear in the witness**, so `t` composes in the exponent and `z` in the scalar field, by Lagrange interpolation. A quorum of nodes can therefore assemble a proof that an ordinary verifier accepts **while no one of them holds the witness**. A general-purpose SNARK has no such structure, which is why collaborative SNARKs run the whole proving algorithm inside MPC.

The statement proved is that applying the committed policy to the committed request yields `key_i`, and that the disclosed winner is the minimum of those. Minimality and membership together say exactly that `v` is the minimum.

**Which policy form.** The verifier rebuilds `ask = ask_level + depth + skew` and `bid = ask_level - spread - depth + skew` from the registered commitments, where `depth` is `slope * qty` and `skew` is `invcoef * inv`. There is no reference-price term in it. That is the rule a circuit generated with `--reference none` computes, and it is the form every figure in this section was measured on: `circuit_bound_proof.json` records the wires the circuit wrote and its `wire_order` carries no `use_ref` and no `ref_mid`. A circuit generated with `--reference anchored`, which is still the flag's default, reaches `ask` through `anchored = ask_level + use_ref * ref` instead, and `shares_from_circuit` refuses that wire rather than proving a different rule than the one the verifier checks. Section 5 shows both forms.

| makers | prove | verify | winner matches the cleartext minimum |
|---:|---:|---:|---|
| 4 | 152 ± 0 (n=15) ms | 173 ± 0 (n=15) ms | True |
| 8 | 307 ± 0 (n=15) ms | 350 ± 1 (n=15) ms | True |
| 16 | 622 ± 0 (n=15) ms | 707 ± 0 (n=15) ms | True |
| 32 | 1259 ± 3 (n=15) ms | 1430 ± 1 (n=15) ms | True |

Linear in the number of makers. That fits a 60-second disclosure or a one-second RFS update; it does not fit under 200 ms.

### Forgeries, rejected

| control | rejected | why |
|---|---|---|
| winner swapped to a non-minimal maker | **True** | the published winner value is not what the commitmen |
| expired maker appears and cannot win | **True** | gated off; winner is maker 5 |
| request nobody can fill answers `no quote` | **True** | every maker gated to the sentinel |
| the winning maker switched off | **True** | maker 5: eligibility is not the conjunction of its t |
| minimality proofs swapped between makers | **True** | maker 0: not shown to be at least the winner |
| minimality for a false winner | **True** | value -1 outside [0, 2^25) |

### Assembled jointly by the nodes --- one opening, not the proof

What is assembled jointly is **one Pedersen opening**: a single scalar dealt to seven nodes and one sigma proof built from a quorum of them. The quote proof's product and bit steps share the linearity that makes this work. Its range proofs do not --- a range proof commits to each bit of the value, extracting bits needs the value, and a node holding a share cannot do that. Assembling the whole proof from shares is MPC, which is what this construction was chosen to avoid, and the range proofs are its dominant cost. So the figures below are a lower bound on a fully assembled proof and not a measurement of one.

| quorum | assemble | an ordinary verifier accepts | no node holds the witness |
|---|---:|---|---|
| 3 | 1.827 ± 0.008 (n=20) ms | True | True |
| 7 | 4.303 ± 0.014 (n=20) ms | True | True |

Below the threshold (two nodes) the assembled proof does not verify; that is checked too.

### The prover's shares are the circuit's shares

The proof is assembled from shares, and until the circuit kept them those shares reached the prover by a route of their own --- nothing said they were the numbers the circuit computed on. A proof about numbers that merely agree with a computation is not a proof about the computation, and this was the largest thing the design asserted rather than showed.

`sint.write_to_file` now makes each node keep its share of the winner, and `rust/qomm-mpc/src/persistence.rs` reads them back. On a seven-party run at *T* = 2 over MP-SPDZ's 128-bit field the shares reconstruct to the value the cleartext reference predicts; every subset of three agrees, two do not recover it, and one flipped bit is noticed. The run ships as a fixture, so the check needs no MP-SPDZ.

| makers | field | rounds | sent per party | median, 1 ms one way |
|---:|---|---:|---:|---:|
| 8 | default 128-bit | 64 | 1.549 MB | 0.568 s |
| 8 | **Ed25519 scalar field** | 104 | 20.791 MB | 0.868 s |
| 16 | default 128-bit | 71 | 3.149 MB | 0.568 s |
| 16 | **Ed25519 scalar field** | 136 | 41.186 MB | 1.269 s |

The price is 1.6 to 1.9x the rounds, 13x the traffic and 1.5 to 2.2x the wall clock.

---

## 5. The pricing rule as a language, and the audit derived from it

The pricing rule is restricted to a small notation with a limited instruction set: it must reference only permitted inputs, must not use a user's identity or address as a pricing input, and must have a bounded output range. These are **static properties of a program**, so they are a checker's job and not a proof's.

Two forms are registered, and which one a maker uses is the `use_ref` bit. **The proof of section 4 is over the second.**

*Anchored* --- `mid` is an offset from the reference price of whichever asset was asked for, so one rule serves every market (`--reference anchored`, the flag's default):

```
# the price rule a market maker registers, and nothing else.
# ask_level is an offset from the reference price of whichever asset was asked
# for, so the same rule serves every market the circuit covers. A maker in a
# market with no usable reference sets use_ref to 0 instead --- see
# quote_absolute.rule.
#
# A maker registers the price it will sell at and the spread it will buy at,
# which is how a maker quotes. The earlier form registered a midpoint and half a
# spread; the two are an affine change of variables, but this one lets the level
# be declared without a sign in the absolute rule and so keeps both prices
# positive there. The spread costs one Bulletproof bucket more --- [2,400] needs
# 9 bits where a half-spread of [1,200] needed 8 --- and that is the whole price
# of the change.
#
# Non-crossing is still an identity rather than a check:
#     ask - bid = spread + 2 * slope * qty >= 2
# so a crossed quote cannot be written, which is stronger than being filtered.
param ask_level[-2000,2000], spread[2,400], slope[0,16], invcoef[0,8], maxqty[1,1000]
param expiry[0,1000000], active[0,1], use_ref[1,1]
state inv[-4000,4000]
input qty[1,400], ref_mid[90000,110000], now[0,1000000]

ask      = use_ref * ref_mid + ask_level + slope * qty + invcoef * inv
bid      = use_ref * ref_mid + ask_level - spread - slope * qty + invcoef * inv
eligible = (qty <= maxqty) and (expiry > now) and (active == 1)
```

*Absolute* --- the maker carries the whole level in `mid` and re-deals it as often as it likes, so nothing is added from the reference table (`--reference none`). This is the form the quote proof's statement covers and the one every figure in section 4 was measured on:

```
# The same price rule for a market with no usable benchmark.
#
# `use_ref` is 0, so nothing is added from the reference table and `ask_level`
# carries the whole price rather than an offset from one. That is what an
# illiquid instrument needs --- a corporate bond has no continuous mid to be an
# offset from --- and it is why `ask_level` is declared wide here and narrow in
# `quote.rule`.
#
# The lower bound is not a style choice. With no reference to lift them, both
# prices are the level minus everything that adjusts it downward:
#
#     spread(400) + slope(16) * qty(400) + invcoef(8) * |inv|(4000) = 38800
#
# so a level under 38801 admits a registration that quotes a negative price, and
# the checker derives exactly that. The floor is set above it. The skew is 82%
# of the total, so the alternative to a high floor is less skew authority, not a
# clamp: clamping both sides costs 328 Bulletproof bits against 176 here, and it
# means buying at the floor when the rule said not to buy at all.
#
# The cost of the width is a range proof: a 12-bit parameter rounds up to a
# 16-bit proof, an 18-bit one to 32. The two files are the two ends of that
# trade, and a maker picks per market rather than the venue picking for all.
param ask_level[40000,200000], spread[2,400], slope[0,16], invcoef[0,8], maxqty[1,1000]
param expiry[0,1000000], active[0,1], use_ref[0,0]
state inv[-4000,4000]
input qty[1,400], ref_mid[90000,110000], now[0,1000000]

ask      = use_ref * ref_mid + ask_level + slope * qty + invcoef * inv
bid      = use_ref * ref_mid + ask_level - spread - slope * qty + invcoef * inv
eligible = (qty <= maxqty) and (expiry > now) and (active == 1)
```

The `use_ref * ref_mid` term survives in both because the checker refuses a rule that declares a value it does not price with; at `use_ref[0,0]` it contributes exactly zero. The circuit does not carry the term at all under `--reference none`, which is why the two agree on the value while disagreeing share by share --- a multiplication re-randomises, so even a sharing of zero is a fresh one.

The instructions are `+ - *`, comparison, `and`, and `min` `max` `clamp` `signed`. There is no division, no loop, no indexing and no attribute access. The surface is a subset of Python expressions parsed with the standard `ast`, and **only the permitted node types pass**. The allowlist is itself the safety argument.

### What the checker derives, with no proof involved

| derived | anchored | **absolute (what is proved)** |
|---|---|---|
| output interval `ask` | (56000, 150400) | **(8000, 238400)** |
| output interval `bid` | (49200, 143998) | **(1200, 231998)** |
| output interval `eligible` | (0, 1) | **(0, 1)** |
| maximum degree in the secrets | 2 | **2** |
| **bit width the circuit needs** | 19 | **19** |

That is what shows the output range is bounded. Both forms need the same 19 bits --- the anchored rule spends them on a reference band that the absolute rule spends on a wider `mid` --- so the justification for the 31 bits chosen by hand does not depend on which one a maker registers. **The same declaration yields both the circuit's width and the content of the audit.**

### The audit is derived

One walk of the same tree produces the value and the proof together. There is no hand-written audit.

| kind of proof | anchored | absolute |
|---|---:|---:|
| bit | 3 | 3 |
| product | 12 | 12 |
| range | 11 | 11 |

Measured on Ed25519: building the audit **28.9 ms**, verifying **32.2 ms**, output identical to cleartext evaluation. A test checks that adding a term to the rule adds the corresponding proof.

### State-update rules are written in the same language

The `s_{i,t+1} = U_i(s_{i,t}, f_t)` form is just another rule. Including saturation at an inventory limit it audits in about **30 ms to prove and 35 ms to verify**. Soundness for `min`/`max`/`clamp` is stated as 'the result is at most each input, and equal to one of them'; the second half follows from a product opening to zero, so which branch was taken never has to be proved.

---

## 6. Multiple assets: hiding which market the request is for

Splitting the MPC job per market would let **which job ran** announce the market. So one circuit serves every market and selects the reference price while it stays secret.

```
ref = sum_a (asset == a) * REF_TABLE[a]
```
That is a secret bit times a public constant, so it costs no multiplication; the cost is one layer of equality tests, as wide as the number of assets. Selecting the row publicly would leak the market at that point.

### What one circuit costs for A markets (16 makers, 31 bits, 1 ms one way)

| assets | rounds | sent per party | median |
|---:|---:|---:|---:|
| 1 | 70 | 3.19 MB | 0.567 s |
| 2 | 70 | 3.2309 MB | 0.567 s |
| 4 | 70 | 3.31269 MB | 0.567 s |
| 8 | 70 | 3.47634 MB | 0.566 s |
| 16 | 70 | 3.80363 MB | 0.566 s |
| 32 | 70 | 4.45816 MB | 0.566 s |

**The round count does not depend on the number of assets.** Where latency dominates, oblivious reference selection is effectively free. Only the traffic grows, by about 0.04 MB per asset.

### Does the trace change with the asset asked for?

| assets probed | rounds | sent | wall-clock spread | distinct answers | all verified |
|---:|---|---|---:|---:|---|
| 8 | [70] | [4.45816] | 0.0500 s | 5 | True |

Rounds and bytes are identical across every asset (True / True). The answers differ per market while the trace does not. The 0.0500 s spread comes from one outlying sample and is the same size as the run-to-run variation seen in other sweeps.

**Boundary**: what is hidden is the market selection inside the MPC. Settling on chain as-is would reveal the market from the asset that moves; secrecy after a trade is not a goal of this stage. Also, when few makers serve an asset the answer is 'no quote', and that fact is itself a hint about how thin the market is. The circuit runs the same shape in that case and returns a sentinel.

---

## 7. Three times: priced, proved, settleable

Allowing settlement before the proof is complete gives up the guarantee, so the three are recorded separately.

| delay | priced | proved | settleable | total | meets an audited 1 s RFS slot |
|---|---:|---:|---:|---:|---|
| 1 ms one way | 940 ± 32 (n=5) ms | +623 ± 1 (n=5) ms | +714 ± 11 (n=5) ms | **2277 ± 39 (n=5)** ms | False |
| 15 ms one way | 4007 ± 77 (n=5) ms | +623 ± 1 (n=5) ms | +709 ± 1 (n=5) ms | **5340 ± 78 (n=5)** ms | False |

**An audited RFS does not make a one-second slot.** After the price comes back, completing the proof takes 623--623 ms and verifying it plus reaching a quorum of receipts a further 709--714 ms --- and neither depends on the delay, so neither shrinks with a closer deployment. At one millisecond one way the total is still 2.28 s.

How to read it: 'priced' includes compiling the circuit and starting the processes on every run, so it is an upper bound. 'Proved' and 'settleable' are the cost of the computation itself and do not shrink with deployment. The remedies are to make the proof lighter in the number of makers --- it is `O(M)` today --- or to set the update interval to what is measured.

---

## 8. Can the number of rounds come down?

Response time is dominated by `rounds x RTT`. So: can the rounds come down? Compiling the circuit one layer at a time decomposes where they go (16 makers, 31 bits).

| stage | rounds | increment | share |
|---|---:|---:|---:|
| inputs, reference lookup and price arithmetic | 10 | — | — |
| + direction selection | 11 | +1 | 2% |
| + eligibility gates | 20 | +9 | 17% |
| + binary tournament | 53 | +33 | 62% |

**The tournament is 62% and the eligibility layer 17%; the price arithmetic is effectively nothing.** Only the sequential depth of the comparisons matters; the width of a layer does not.

### Where the rounds go at runtime, and whether security moves them

The compiler's count is a property of the circuit. What the parties actually do is a property of the protocol too, and the engine keeps it --- reading it required linking MP-SPDZ as a library rather than parsing what it prints.

| protocol | channel | rounds | share | sent per party |
|---|---|---:|---:|---:|
| malicious-shamir | Partial broadcasting | 49 | 70% | 0.367 MB |
| malicious-shamir | Sending/receiving | 10 | 14% | 1.845 MB |
| malicious-shamir | Receiving directly | 6 | 9% | 0.000 MB |
| malicious-shamir | Broadcasting | 5 | 7% | 0.000 MB |
| malicious-shamir | *total* | 70 | 100% | 3.313 MB |
| semi-honest-shamir | Sending/receiving | 49 | 79% | 1.136 MB |
| semi-honest-shamir | Sending to all | 7 | 11% | 0.001 MB |
| semi-honest-shamir | Receiving directly | 6 | 10% | 0.000 MB |
| semi-honest-shamir | *total* | 62 | 100% | 1.143 MB |

The opening channel --- the comparison chain --- is **49 rounds under both protocols**. Dropping malicious security takes rounds out of everything else (70 to 62) and cuts bytes by **2.90x**. The security model is paid in bandwidth; the latency is owed to depth either way.

### What did not work

| lever | result |
|---|---|
| preprocessing batch 10000 | 70 rounds, 3.31 MB, 3.623 s |
| preprocessing batch 1000 | 124 rounds, 3.07 MB, 4.329 s |
| preprocessing batch 100 | 700 rounds, 3.22 MB, 14.092 s |

A smaller batch means more batches and so more rounds. At the default of 10,000 the preprocessing fits in one. Generating edaBits online was 23x worse when measured. Separating offline from online is measured in its own section below.

### Offline and online, separated

An earlier version of this file said this could not be measured because `Fake-Offline.x` does not produce malicious-Shamir preprocessing. That was wrong. It does --- `./Fake-Offline.x 7 --threshold 2 --default 200000 -lgp 128` writes `Player-Data/7-MSpT2-128/` for malicious Shamir at T=2 and `7-SpT2-128/` for Shamir, which `atlas-party.x` reads too because `AtlasShare` does not override `type_short`. `-F` then makes a party take its correlated randomness from disk, so what is left on the wire is the online phase.

| protocol | preprocessing | rounds | sent, party 0 | sent, all | time |
|---|---|---:|---:|---:|---:|
| malicious-shamir | in protocol | 62 | 2.738 MB | 16.184 MB | 55.8 ms |
| malicious-shamir | from files | 44 | 0.436 MB | 3.049 MB | 25.5 ms |
| atlas | in protocol | 100 | 0.614 MB | 2.474 MB | 77.7 ms |
| atlas | from files | 91 | 0.131 MB | 0.900 MB | 56.3 ms |

All four verify against the cleartext reference. The online phase is **16% of party 0's bytes** and 19% of the global total --- the two differ because a party's share of the traffic depends on where it sits in the reconstruction --- against 71% of the rounds. The byte saving was predicted at "at least 30%" and is 84%.

**What this does not show.** `Fake-Offline.x` is a trusted dealer: it writes every party's share from one process that knows all of them, which is not a protocol any deployment can run. So this measures the *size of the online phase* and not the cost of putting the randomness there. A real offline phase among the nodes costs more than the dealer did, and the saving is moved off the critical path rather than removed.

Measured on host-c; the absolute times are not comparable with the host-a tables above and the ratios are the result.

### What buys bandwidth but not rounds

| lever | rounds | sent | wall clock, 15 ms one way |
|---|---:|---:|---:|
| baseline | 70 | 3.313 MB | 3.677 s |
| public maker assets | 70 | 2.658 MB | 3.573 s |
| gates moved to the audit | 69 | 2.469 MB | 3.626 s |
| both | 69 | 1.815 MB | 3.475 s |

Making the market each maker serves public turns the asset check into a **public index** into a secret one-hot vector, and the equality test disappears. Expiry and the active flag are already proved by the registration audit, so the circuit need not pay for them twice. Together the traffic falls **45%**, but these are amounts of work inside the same layer, so **the rounds move only from 70 to 69**.

### What did work: more requests in one job

Rounds are **a property of the job, not of the request**. Sharing the same comparison layers across Q requests divides the rounds per quote by Q. That fits the fixed-cadence slot design exactly.

| requests Q | rounds | rounds per quote | sent | job wall clock | per quote |
|---:|---:|---:|---:|---:|---:|
| 1 | 69 | 69.0 | 1.8 MB | 3.425 s | **3425 ms** |
| 2 | 69 | 34.5 | 3.6 MB | 3.677 s | **1839 ms** |
| 4 | 78 | 19.5 | 6.9 MB | 4.029 s | **1007 ms** |
| 8 | 93 | 11.6 | 13.6 MB | 4.594 s | **574 ms** |
| 16 | 123 | 7.7 | 26.9 MB | 6.034 s | **377 ms** |
| 32 | 177 | 5.5 | 55.1 MB | 9.090 s | **284 ms** |

**From Q=1 to Q=32, the rounds per quote go from 69 to 5.5, a factor of 12.5, and the time per quote from 3425 ms to 284 ms, a factor of 12.1.**
Even at 15 ms one way (30 ms RTT), 32 requests together reach about 0.28 s each.

**Boundary**: this is throughput, not one user's wait. The Q=32 job itself takes 9.1 s. A user's wait is capped by the slot period, so Q is chosen to match the arrival rate: at three arrivals a second, a one-second slot fills naturally at Q=3.

---

## 9. What is not built yet

| item | state |
|---|---|
| emit MPC computations and receipts on a fixed cadence | **measured** (sections 1, 2) |
| make a real request and a cover leave the same trace | **measured**. Both the MPC job (1) and the user-to-node path (3) |
| hide which market a request is for | **measured** (6). The round count does not depend on the asset count |
| fixed-cadence sending and a relay network to hide a user's origin | **measured** (3). Multiple hops are implemented but in-process |
| fix the eligible-maker set and detect omissions | **measured** (2) |
| detect and slash double signing, stale state and selective stalling | **measured** (2) |
| check the computation with a proof every time | **measured** (4) |
| have the MPC nodes jointly build one verifiable proof | **measured** (4) |
| restrict the form of an approved pricing rule and audit it at registration | **measured** (5). The DSL's checker and the derived audit |
| a ZK audit of the state-update rule | **measured** (5). Another rule in the same language, same machinery |
| the three times: priced, proved, settleable | **measured** (7), and it does not make a one-second RFS slot |
| register a digest of the approved circuit and detect substitution | **built**. `rust/qomm-dsl/src/registry.rs`. The digest covers the expressions, the declared ranges, the circuit and the required bit width. Substituting a secret parameter passes; substituting the rule is refused |
| relays over a real network, multiple hops | **measured** (3). Each hop is a real socket, about 4.4 ms per hop |
| identify a node that emitted an inconsistent partial value | **built**. The joint proof's record names the node whose partial value does not agree with its own share |
| measure offline/online separation | **measured**. `artifacts/prep_split.json`. With preprocessing on disk the online phase is 16% of party 0's bytes --- 19% of the global total --- and 71% of the rounds |
| secrecy after a trade, where settlement reveals market and size | **out of scope** for this stage |
