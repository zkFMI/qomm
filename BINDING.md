# Binding what was computed to what was committed

The one gap in the middle of the stack, the two ways to close it, and what each
one costs. Every figure here is measured on `host-a` unless it says otherwise,
and every arm was verified against the cleartext answer before its time was
taken --- a timing from a run that computed the wrong thing is not a timing.

`DEPLOYMENT.md` points here from section 0; this used to be section 0.6 of it,
and grew until it was half of a document about where to put nodes.

---

## 0. What is open

A maker deals its policy to the seven computing nodes as shares. A node can
check its own share against the dealer's commitment before it computes
(`check_share`). **What nothing forces is that the value the node then puts into
MP-SPDZ is the share it checked.**

`rust/qomm-transport/src/roles.rs` says so in the code rather than leaving it to be
discovered: the signature on a dealt share buys **detection and attribution, not
prevention**. A node that substitutes an input can be shown afterwards to have
done it.

**Whether that is enough is a question about who the nodes are.** Seven KYB'd
legal entities with a bond to slash and a jurisdiction to be sued in are held by
attribution; prevention buys them little. Seven anonymous operators are not held
by anything, and everything below is for that case.

The same gap sits under three modules, and it is one gap rather than three.
`rust/qomm-proofs/src/policy_audit.rs` states it exactly:

> these shares are not the shares MP-SPDZ consumes. MP-SPDZ works over its own
> prime field, so an end-to-end binding needs the computation to run over the
> same field as the commitments, **or a commit-and-prove link between the two**.

Those are the two ways. They are sections 2 and 3.

**And the mechanism is not new, which is worth saying at the top rather than
discovering in review.** Publicly Auditable MPC (Baum, Damg{\aa}rd, Orlandi,
SCN 2014) has input providers publish Pedersen commitments whose homomorphic
operations mirror what the protocol does to the values, so an auditor replays
the function on the commitments. That is the construction the quote proof
instantiates. What is here is an application of it and the cost of running it,
not the mechanism.

---

## 1. Why the field is what stands in the way

`threshold_sigma` is how the nodes prove anything about a value none of them
holds. A sigma response is

    z = k + c * w

which is **affine in the witness**, so each node computes `z_i = k_i + c * w_i`
from its own share and the pieces Lagrange-combine into `z` --- in the **scalar
field of the group**. That is the whole method, and it is why this design uses
sigma protocols instead of a general-purpose SNARK.

It works only if the shares are over the group order. MP-SPDZ works over its own
prime, so today the proof is assembled from shares nobody can show are the
computation's.

**Two proofs need this and two do not.** `rust/qomm-proofs/src/policy_audit.rs` and `rust/qomm-proofs/src/state_audit.rs`
never import `threshold_sigma`: a maker proving something about its own policy
knows the witness and proves alone. The quote proof and zkPI are assembled from
node shares, and they are where the field matters --- and they are also the
system's headline claim, that the returned price was the minimum and anyone can
check it.

---

## 2. Matching the field

Run MP-SPDZ over the ed25519 group order and the Lagrange combination becomes
valid as written. No new cryptography; the existing code starts meaning what it
says.

Both arms in one session on one machine, every arm verified (M=16, 31 bits,
4 assets, N=7):

| | rounds | global traffic | wall @15 ms | wall @120 ms |
|---|---:|---:|---:|---:|
| MP-SPDZ default field | 64 | 19.37 MB | 3.621 s | 27.603 s |
| ed25519 scalar field, 253 bits | **64** | **38.73 MB** | **3.877 s** | **29.179 s** |
| ratio | **1.00x** | **2.00x** | **1.07x** | **1.06x** |

**Rounds do not move. Traffic is exactly 2.00x, which is the element width going
from 16 bytes to 32 and nothing else. Wall clock is 7% at 15 ms and 6% at
120 ms** --- cheaper cross-region, because the round count is unchanged and the
round trip is what dominates there.

**So matching the field is affordable, and that decides most of this document.**
`threshold_sigma` assembles, the quote proof is publicly verifiable, and it costs
six to eight per cent.

### 2.1 The seven-times error that this section used to report

The first version of this measurement reported **2.02x rounds and 14.3x
traffic**, and everything downstream was reasoned from it: which field to take,
whether the input check was worth building, what the throughput was
cross-region, whether a different group or a smaller field would help.

**All of it was one wrong flag.** The circuit was compiled with `-P <prime>`.
MP-SPDZ prints, in the compile output of every such run:

> `WARNING: --prime/-P activates code that usually isn't the most efficient
> variant. Consider using --field/-F and set the prime only during the actual
> computation.`

That warning was in the output every time and was not read. Compiling with
`-F <bits>` and giving the prime to the virtual machine at run time is the
supported path, and it removes the entire penalty.

**Three things were then measured against a problem that did not exist**, and
they are worth keeping for what they say about the tool rather than about the
question:

- **edaBits** made it two orders of magnitude worse --- 2,920 MB on the default
  field, 6,257 MB on the matched one. They are preprocessing, and this harness
  measures one phase, so their generation lands in the run with nowhere to hide.
  A real offline stage is still unmeasured.
- **Probabilistic truncation** changed nothing at all, to the decimal.
- **`audit_gates` and `public_maker_assets` together** took 277.3 MB to 137.3 MB.
  Real levers, and they still work --- they are just no longer needed for this.

And the literature was searched for a way around a barrier that was not there.
**Rabbit** (Makri, Rotaru, Vercauteren, Wagh, FC 2021) removes the statistical
security requirement that makes comparison cost scale with the field, "allowing
MPC protocols to be run in a field of arbitrary size" --- and MP-SPDZ already
implements it, gated on the prime being close to a power of two, which the
ed25519 order is by a margin of `2^-127`. The fast path was available the whole
time; the compile flag was not letting it be reached.

**The lesson is not about MP-SPDZ.** A seven-times figure was published to three
repositories and a slide deck, and used as the premise of four subsequent
decisions, while the tool printed the reason in plain English on every run.

## 3. Checking the inputs instead

`rust/qomm-harness/src/bin/run_input_check.rs`. The dealer already publishes a commitment per input. Once
the inputs are fixed, public coefficients are derived from those commitments by
Fiat--Shamir, the circuit computes

    s = sum_j c_j v_j + r

for a committed mask `r`, and opens `s`. Pedersen commitments add, so the same
coefficients combine the commitments into one that `s` must open. A node that
feeds the circuit `v_j + e_j` shifts `s` by `sum_j c_j e_j`, and the coefficients
it would have to satisfy did not exist when it chose `e`.

**Public coefficient times secret share is local**, so the combination costs no
communication; the opening is the round.

### 3.0 The width budget below is now history, and why

Everything in section 3.1 --- 164 bits, the mask spending its forty bits twice,
the narrow-coefficient trade with its ceiling at `2^-34` --- exists for one
reason: the check was taken **over the integers**, so the opening had to avoid
reducing in the MPC field *and* in the group.

Writing the security proof found that the coefficients were also being fixed
before a node chose what to feed, which made the check unsound
(`artifacts/coefficient_timing_flaw.json`). The correction --- draw the
challenge after the input phase, take its powers, work **modulo the MPC prime**
--- fixes the soundness and **deletes the budget at the same time**. Nothing has
to avoid reducing when both sides are modulo `p`, and the mask becomes one
uniform field element.

Measured: **one extra round and 0.39% more traffic** than the aggregate check,
at the matched field (`artifacts/sound_check.json`). Soundness goes from
`2^-42` to about `2^-245`.

**What follows is kept because it is what the cost measurements were taken
against**, and because the reasoning about where a check can run is still the
reasoning --- it just has a different answer now.

### 3.1 The width budget, which decides where it can run

The whole argument is that **the same integer appears on both sides**, so the
opening must not reduce in either field. Two things have to fit, and the second
is the one that bites: `r` is an input like any other, dealt through
`roles.split` additively over the integers with `SLACK_BITS` of room per share.
**The forty bits are spent twice** --- once hiding the combination in the
opening, once hiding each share from its node.

    field needed = value_bits + challenge_bits + log2(n) + hiding_gap
                              + share_slack + log2(nodes) + 2

At 32-bit values and 166 inputs that is `challenge_bits + hiding_gap <= 41` in a
127-bit prime. Forty-bit coefficients need **165 bits** and do not fit.
**Six-bit coefficients need 126 and do.**

Narrow coefficients lose soundness, and **repetition buys it back for free in
rounds** --- independent combinations wait on nothing and open together. Seven
six-bit rounds are `2^-42`, clearing the `2^-40` the rest of the stack works to.

### 3.2 What repetition cannot buy back

The hiding. More repetitions dilute the gap while buying soundness, so the curve
has a peak --- **about `2^-34`, at two to four coefficient bits with eleven to
twenty-six repetitions** --- and `2^-40` is out of reach at 127 bits anywhere on
it. The shipped six-by-seven setting sits at `2^-32`. `narrow_tradeoff` tabulates
the curve and a test holds the peak.

**This is the honest cost of running the check in the narrow field**, and it is
the only one.

### 3.3 Measured

Cryptography, over 166 inputs, ed25519, `host-a`: **16.4 ms to build, 13.9 ms to
verify**, which is 204 scalar multiplications. 34 tests: a substituted input
caught at every position and error size, two errors that try to cancel caught,
coefficients that move when the commitments move, an opening that differs every
time.

In the circuit, both arms verified:

| | rounds | global | wall @15 ms |
|---|---:|---:|---:|
| default field, no check | 64 | 19.37 MB | 3.583 s |
| **default field, input check (6 bits x 7)** | **65** | **19.38 MB** | **3.631 s** |
| 177 bits, input check | 125 | 150.84 MB | 6.377 s |
| group order, no check | 129 | 277.26 MB | 7.783 s |

**One round, six kilobytes, forty-seven milliseconds.**

---

## 3.9 Built, and run end to end

Sections 2 and 3 measured the two routes. This is the first one wired up:
`rust/qomm-transport/src/binding.rs`, `rust/qomm-harness/src/bin/run_binding_chain.rs`, `make binding-chain`.

The fix is an identity rather than a comparison, which is the part worth
stating. `build_inputs` takes a `deal_hook`, so the party that commits to a
value is the party that shares it, in the one pass that writes the party files.
Nothing can be committed and not dealt, or dealt and not committed, because the
two are the same object --- and the test that says so is `==` between the party
file and the dealer's shares rather than a check that two dealings agree.

The circuit changes by one line. `secret_input()` was `sum(get_input_from(p))`
and is now `sum(LAGRANGE[p] * get_input_from(p))` with public coefficients
interpolating at zero from **all `n` points**, not `t+1`. Public times secret is
local, so the round count and the input count are both untouched.

Measured here, `host-c`, one market, `n=8` makers, `N=7`, `T=2`, 31-bit values,
both arms verified against the cleartext reference:

| | answer | rounds | global traffic |
|---|---|---:|---:|
| default field, additive inputs | `(99970, 1)` | 57 | 9.3038 MB |
| **group order, Shamir inputs** | **`(99970, 1)`** | **57** | **18.6019 MB** |
| ratio | same | **1.00x** | **1.9994x** |

**Rounds do not move and traffic is the element width and nothing else**, which
is what section 2 measured on `host-a` and what was predicted in
`artifacts/binding_chain_prediction.json` before any of this was written. Wall
clock is not comparable to section 2's 1.07x: that was at 15 ms of simulated
delay, where round trips dominate; at zero delay on a laptop the field width
shows through and the same pair is 0.197 s against 0.315 s.

The chain, and what each arm does (`artifacts/binding_chain.json`):

| arm | outcome |
|---|---|
| honest | 86 values committed and shared in one pass; 602 share checks, none failing; a range proof on `half` against the band `[1, 200]` verifying **against the commitment that was dealt**; circuit `got=(99994, 5) want=(99994, 5)` over the 253-bit prime |
| a dealer that deals what it did not commit | **caught at party 3, position 8, before anything is computed** |
| a node that feeds something else | **not caught here, and the artifact says so** --- a substituted input is a valid share of a different number and every commitment still opens. That is section 3's check, which catches the node where this catches the dealer |

One share check is **0.167 ms** --- three point exponentiations against the
coefficient commitments plus one commitment --- against a prediction of 0.19 ms.
Seventy fields across seven nodes is 13 ms, next to the range proofs the audit
already pays.

**They compose --- and the first time they were run together they did not.**

The generator emitted two definitions of `secret_input`. The input check's
recorded each party's share into `check_store`; Shamir inputs emitted another
immediately after that applied the Lagrange coefficients, and the second
silently replaced the first. So asking for both --- which is the arrangement
this section recommends --- produced a circuit whose check ran over an array
nothing had written to. Nothing failed, because a check over zeros passes.

The table below was measured in that state. Its numbers are unaffected: after
the fix the same four arms reproduce them to four decimal places, because
recording a share into an array is local and costs no communication. What was
wrong was the property, not the price. **The rows are the cost of the
machinery; they became evidence that the composition works only after the
generator emitted one definition carrying both options.**

Six arms, all verified against the cleartext reference on one market:

| inputs | field | check | rounds | global traffic |
|---|---|---|---:|---:|
| additive | default | none | 57 | 9.3038 MB |
| additive | default | aggregate | 58 | 9.3101 MB |
| additive | default | per-party | 59 | 9.4448 MB |
| Shamir | group order | none | 57 | 18.6019 MB |
| Shamir | group order | aggregate | 58 | 18.6145 MB |
| **Shamir** | **group order** | **per-party** | **59** | **18.8838 MB** |

Re-measured on `host-c` after the fix: 57 / 9.3038 MB, 59 / 9.4448 MB,
57 / 18.6019 MB and 59 / 18.8838 MB --- the same four figures, which is what a
round count and a byte count should do, being counts rather than timings.

The aggregate check is one round and about six kilobytes, and the per-party one
is **two rounds and 0.28 MB** --- one opening a node rather than one for all of
them, which is what buys naming the node instead of only detecting that
somebody substituted. That is the price of the difference and it is worth
stating next to it, because the earlier table quoted the aggregate's cost for a
check that is not sound.

`--check-mode aggregate` now returns non-zero without
`--unsound-check-for-measurement`. The rows above are how it was measured.

**And the state audit is bound too.** `verify_chain_bound` requires every step
to start from the commitment the input dealer published for that maker's
inventory, so an impeccable inventory chain over a book the circuit never saw is
refused --- with its own `ChainError` variant, kept apart from the three that
were there because it calls for a different response. The test that would have
caught it asserts both halves: the unbound chain verifies and the bound one does
not.

---

## 4. Which one to take

| | binds the inputs | quote proof publicly verifiable | cost |
|---|---|---|---|
| default field | no | no | --- |
| default field + check | yes | no | +1 round, +6 KB |
| **group order (253 bits)** | **yes** | **yes** | **2.00x traffic, 1.07x wall** |
| group order + check | yes | yes | the above, +1 round |

**Take the group order.** It is six to eight per cent of the wall clock and
twice the bytes --- 5.5 MB a node a quote against 2.8 --- and it buys the
publicly verifiable quote proof, which is the system's headline claim.

**The input check is no longer the cheap alternative to that.** It is still
worth having and still costs one round on the matched field (measured: 65 rounds
against 64, +0.013 MB), but as a **fast pre-check**: it catches a substituted
input at once rather than when a 317 ms proof fails to verify. Defence in depth
rather than a substitute.

The narrow-field version of the check in section 3 remains the answer for a
deployment that skips the quote proof entirely --- profile A in
`DEPLOYMENT.md`, where latency is everything and correctness proofs are not
produced.

## 4.5 The commitment scheme as a choice, and what a different one costs

`rust/qomm-zk/src/lib.rs` made the *group* pluggable, which covers every discrete-logarithm
scheme and no others. The Rust commitment traits move the seam up to the commitment, because
the interesting alternative is not another group:

    commit, add, scale, negate, zero, encode, equal, scalar_modulus

`PedersenScheme` delegates to the curve. `VoleScheme` is a linearly homomorphic
commitment from a VOLE correlation --- `M = K + Delta*x` over a prime field,
where the prover holds `(x, M)` and the verifier `(K, Delta)`. Every operation is
field arithmetic instead of a scalar multiplication.

`rust/qomm-harness/src/bin/run_input_check.rs` is ported to the seam and runs unmodified on both, which is what
the seam is for. Measured on `host-a`, 30 repeats:

| | one `scale` | build, 166 inputs | verify, 166 inputs |
|---|---:|---:|---:|
| Pedersen (ed25519) | 68.1 us | 16.39 ms | 13.90 ms |
| **VOLE** | **0.605 us** | **0.31 ms** | **0.36 ms** |
| ratio | **113x** | **53x** | **39x** |

**But they do not promise the same thing, and the interface says so.** Pedersen
is *publicly verifiable*: a commitment is a group element anyone can hold,
combine and check, which is what a verifier who was not present needs. The VOLE
commitment is *designated verifier* --- only the holder of `Delta` can check an
opening, and nothing is published.

**Making it public is what VOLE-in-the-Head does** (Baum et al., CRYPTO 2023),
for about twice the communication of the designated-verifier protocol, and
**that transform is not implemented here**. What the table measures is the
commitment underneath it.

Why it is worth measuring anyway: VOLE-in-the-Head has **no group**, so there is
no scalar field for the MPC to match and section 2 stops being a question at all;
it is post-quantum from symmetric primitives, so section 6 stops being one too;
and FAEST signatures at 5.6 to 6.6 kB are the same order as the 4,960-byte sigma
proofs this stack already produces. `eprint 2026/337` combines it with
publicly auditable MPC directly. **This is the direction to read next, and the
seam is what makes trying it a substitution rather than a rewrite.**

---

## 4.6 VOLE-in-the-Head, implemented, and what the 113x became

The transform is now in `rust/qomm-harness/src/voleith.rs`, so the question above is answered by a
measurement rather than by a citation. What it does, in the order it happens:

1. `repeats` GGM trees of `2^depth` leaves each. Each leaf seeds a vector of
   `n` field elements. The prover knows every leaf.
2. `u = sum_x t_x` and `V = -sum_x x*t_x`. The prover publishes `d = w - u`,
   which is what commits the witness.
3. `Delta` for each tree comes from Fiat--Shamir over everything already
   published, and the prover opens all leaves but `Delta` by revealing the
   `depth` sibling seeds on the co-path.
4. The verifier rebuilds those leaves and forms
   `Q = sum_{x != Delta} (Delta - x) t_x = Delta*u + V`, folds in `d`, and
   checks the linear combination against the opening.

Guessing `Delta` is the only way to cheat, so soundness is `2^-depth` per tree
and the default 8 x 16 is 128 bits --- the parameter set FAEST measures as its
fastest.

**Both arms prove the same statement and both are publicly verifiable**, which
is what makes the comparison mean anything. `host-a`, n=30, 167 inputs:

| | prove | verify | proof |
|---|---:|---:|---:|
| Pedersen (ed25519) | 18.64 ms | 15.48 ms | 5,440 B |
| **VOLE-in-the-Head** | **73.06 ms** | **69.94 ms** | **45,616 B** |
| ratio | **3.93x** | **4.52x** | **8.39x** |

**So the 113x was not an advantage over Pedersen. It was the price of not being
checkable.** A designated-verifier VOLE commitment is a field multiply, and a
Pedersen commitment is a scalar multiplication, and that gap is real --- but
only the holder of `Delta` can check the cheap one, and the party that has to
check is a regulator who was not there. Buying the check back costs 3.9x on the
clock and 8.4x on the wire, in the other direction.

### What is actually expensive, which is not the trees

The proof breaks down as:

| part | bytes | |
|---|---:|---|
| VOLE consistency corrections | **40,080** | **88%** |
| witness correction `d` | 2,672 | the commitment itself |
| co-paths | 2,048 | 16 trees x 8 seeds |
| punctured leaf commitments | 512 | |
| tags, opening, root | 304 | |

**The all-but-one openings are 4% of the proof.** The 88% is `repeats - 1`
corrections of `n` field elements each, and they are there because each tree
produces its own `u` and the witness has to be committed against one of them.

That term is why the paper's "2x the designated-verifier communication" does not
carry over. **Over `F_2`, where FAEST lives, those corrections are bits.** Over a
127-bit prime, which is where this stack's arithmetic lives, they are 16-byte
elements, and the same construction with the same parameters produces a proof
`8*127` times larger per element. The computation moves for the same reason:
17.8 MB of PRG output against FAEST's 819 kB at identical tree parameters,
because a leaf here expands to `n` field elements rather than `n` bits.

**The finding generalises past this module.** VOLE-in-the-Head is cheap when the
witness is bits. Every cost it has scales with the width of a witness element,
and a market-making policy is not bits.

### The trade-off curve, which reproduces a published one

Deeper trees mean fewer repetitions for the same 128 bits, hence fewer
corrections --- bought with `2^depth` PRG calls each. At 167 inputs:

| depth | repetitions | proof | hashes |
|---:|---:|---:|---:|
| 4 | 32 | 89,136 B | 1,024 |
| **8** | **16** | **45,616 B** | **8,192** |
| 12 | 11 | 32,080 B | 90,112 |
| 16 | 8 | 23,856 B | 1,048,576 |

FAEST's table 2 has the same shape on the same axis --- 7,506 B at `q = 2^7`
down to 5,559 B at `q = 2^11`, for 4.2x the signing time. Getting a published
curve back out of a harness built for a different statement is the reason to
believe the harness.

### The escape, which is smaller than it first looked

An earlier version of this section said section 6.1 of the paper replaces the
repetition code with a linear code, that the correction becomes
`ceil(n/k_C) * (n_C - k_C)` elements against `n_C` trees, and that the best MDS
parameters `[31, 16, 16]` give **10,816 B for 15,872 hashes** --- 4.2x smaller
for 1.9x the computation. The arithmetic is right and the conclusion is not,
because a general code is not a drop-in for the repetition code here.

The paper says so in the first paragraph of section 6.1. With the repetition
code the commitment is linearly homomorphic for messages in `F_p`; with a
general code it is homomorphic only *across* the `k_C`-blocks, and *"linear
operations must be applied across the vectors"*. The statement this stack
proves is one inner product with a distinct coefficient per value, which is a
combination **within** a block. Splitting it into `k_C` per-column checks is
what the general code permits, and then each column is protected by a single
entry of `Delta`: soundness falls from `|S|^-d_C` to `|S|^-1`, which at
`depth = 8` is 2^-8. The distance stops a prover opening to a *different
codeword*; it does not stop one lying about a per-column opening value.

Recovering the within-block combination is what protocol `Pi_2D-LC` (figure 6)
is for, and it charges: the subspace VOLE is called for `2l+2` rows rather than
`l`, and the prover additionally opens `S = R + U*Delta'`, an `(l+1) x n_C`
matrix. That is the *"around 2x overhead on standard VOLE-based protocols over
large fields"* the paper's introduction states, and it was missing from the
figure above. Counting figure 6's messages, the best parameters move to
`[47, 32, 16]` and the proof to **18,896 B for 24,064 hashes**: still 2.4x
smaller than the measured repetition-code proof, for 2.9x the computation, and
3.5x Pedersen's rather than 2.0x.

**Neither is implemented.** `rust/qomm-harness/src/bin/run_voleith.rs` returns both branches ---
`code_swap_only` and `protocol_complete` --- so the distance between what was
measured and what the construction reaches is a number, and so is the distance
between the two ways of counting it.

### What it buys, which is real and is not speed

Three things, and none of them is on the table above.

**No group**, so nothing for the MPC field to match and section 2 stops being a
question. **Symmetric primitives only**, so section 6 stops being one.
**The commitment is smaller than Pedersen's** --- 2,672 bytes against 5,344,
because a correction is a 16-byte field element and a commitment is a 32-byte
group element. It is the *proof* that is large, not the commitment.

### One opening, and the implementation refuses a second

After `Delta` is published the correlation binds nothing, so a second statement
about the same commitment proves nothing. Baum and Zok (`eprint 2026/337`)
formalise this as *one-time* linear homomorphism and buy a second opening by
committing to the opening in the random oracle. `Prover.prove` raises
`OneTimeError` rather than documenting the restriction, because a scheme that
silently permits an unsound operation is worse than one that lacks the feature.

**This is the property that would bite hardest in deployment.** Pedersen
commitments to a maker's policy are opened against every quote for the life of
the policy. VOLEitH commitments cannot be, so a policy would have to be
recommitted per quote or carried forward through delayed openings, and neither
is free.

### The caveat on the clock, stated as a bound

Pedersen runs on libsodium through PyNaCl, which is C. VOLEitH runs on
`hashlib`, also C, with the field arithmetic in CPython. Measured on the same
host: **the XOF output alone is 30.4 ms of the 73.06**, and the XOF plus the
packing is 54.3 ms. So a compiled implementation that made everything except the
PRG free would still land near 30 ms, and the honest reading of the 3.93x is
**somewhere between 1.6x and 3.9x, with the language accounting for at most the
difference**. The byte counts have no such caveat.

---

Only `rust/qomm-harness/src/bin/run_input_check.rs` is ported to the per-value seam. The rest of the proof layer still
takes a `Pedersen` directly, and porting it would buy nothing further: what the
comparison needed was a second seam at the level of a statement, and that is
what `LinearProofScheme` is.

---

## 5. Would a different group be cheaper? No

**The field size is what costs, and 128-bit security forces about 256 bits of
scalar field for any discrete-log group**, elliptic curve or multiplicative. No
discrete-log scheme shrinks the MPC field.

What the group does change is the speed of the proofs, and `zk_bench.json`
already had it --- one host, one proof, seven fields, seven parties:

| backend | prove | verify | share check |
|---|---:|---:|---:|
| ed25519 | 14.4 ms | 15.5 ms | 7.6 ms |
| modp multiexp | 4,865.6 ms | 7,358.6 ms | 1,419.0 ms |
| ratio | **338x** | **475x** | **187x** |

A non-curve group is worse on one axis and neutral on the other.

**The one direction that could shrink the field is lattice commitments**, whose
security comes from dimension rather than modulus size. Section 6 is why that
does not simply work.

---

## 6. Post-quantum

**Scope first, because the two halves are not the same difficulty.**

| | who proves | which | cost |
|---|---|---|---|
| **single prover** | the maker, knowing its own secret | policy audit, state audit, range proofs, the input check | **size only** |
| **threshold-assembled** | seven nodes, about a value none holds | **quote proof**, zkPI | **structural** |

**The confidential computation is already post-quantum.** Shamir with an honest
majority is information-theoretically secure; there is no assumption to break
and the MPC layer needs nothing.

**And the next part is nearly as favourable.** Pedersen is *perfectly* hiding and
only computationally binding, so a quantum adversary can open a commitment to a
value it does not hold and can **never learn the value**. What quantum takes away
is the ability to prove, never the ability to hide.

### 6.0 That last sentence was consoling, and it is only half true

Losing the ability to prove is not a mild loss when the thing being proved is an
auction, and the loss does not need a quantum computer --- it needs time.
Rivinius, Reisert, Rausch and K{\"u}sters (IEEE S&P 2022, appendix H) put it
directly, against exactly this construction:

> a malicious actor knows a random share `[r]_i` that will be adjusted to fit
> another party's input. They also know a commitment for this share. If they now
> manage to produce a decommitment for the same commitment but with the
> decommitment `[r']_i := [r]_i - c`, they could use this (undetectedly) in the
> protocol and influence another party's input ... **In an auction, this could be
> used to reduce other parties' bids and increase the chances of winning.**

**A maker's policy commitment in this stack is not opened once and discarded.**
It sits for the life of the policy and is opened against every quote, and the
mechanism is an auction. So the binding assumption is not being asked to hold
for the length of a protocol run; it is being asked to hold for as long as the
commitment is live, against an adversary with a direct financial motive to break
it and a quantifiable prize for doing so.

**This is a better argument for the post-quantum direction than the generic one**,
because it does not require the adversary to have a quantum computer at all ---
only enough compute before the commitment is used. It is the reason section 4.6's
73 ms and 45,616 bytes are worth paying attention to even though they are worse
than Pedersen on both axes.

Their own first argument against Pedersen --- that widening the field to hold a
group order widens the BGV plaintext space with it --- **does not transfer**.
There is no somewhat-homomorphic encryption here: honest-majority Shamir has no
lattice preprocessing whose modulus could blow up, which is why section 2 measures
2.00x where they tabulate something much worse. **Both numbers are right about
different protocols**, and the corollary points the other way too: 2.00x is not
evidence that matching the field is affordable for anybody else.

### 6.1 Three obstacles, in increasing difficulty

**Size.** A sigma proof here is `4,960` bytes a step, measured
(`state_audit.json`). Lattice equivalents are kilobytes to tens of kilobytes ---
from the literature, not measured here. Bandwidth, and the design does not
change.

**Rejection sampling.** A lattice response `z = y + c*s` leaks `s` unless the
prover sometimes discards it and restarts, and **the abort test is on the
combined `z` rather than on any share**. The nodes would have to reconstruct `z`
to decide, and a rejection then arrives *after* the leak it exists to prevent.
Each retry needs fresh randomness across all nodes, at two to seven expected
attempts. A one-shot assembly becomes an interactive protocol.

**Shortness against Shamir**, which is the structural one. Lattice soundness
needs a **short** witness. Shamir shares are uniform in `Z_q`, Lagrange
coefficients are arbitrary elements of it, and the combination is literally

```python
z_value = sum(coefficients[p] * partials[p][0] for p in partials) % order
```

--- short things multiplied by large things and reduced. **Shortness does not
survive.** Not a tunable parameter: a mismatch between how Shamir reconstructs
and what lattice soundness requires.

### 6.2 Ways out, and their price

**Replicated secret sharing** keeps shortness, because reconstruction is a plain
sum with `{0,1}` coefficients. At seven nodes and `T=2` that is `C(7,2) = 21`
shares with `C(6,2) = 15` held by each party --- fifteen times the per-party
storage and the multiplication cost that follows --- and it does not scale past
small `n`. **Small-coefficient sharing schemes** exist and trade against
threshold flexibility. **Threshold lattice signatures** are active research whose
known constructions are heavier and need more rounds.

### 6.3 What the signatures cost, which is size and not speed

ML-DSA signs about as fast as Ed25519 in optimised implementations. The bytes
land on the maker update path. Sizes are the standard's, checked against an
implementation; **no timing is claimed**, because the only implementation
available here is pure Python.

| signature | one full policy update | vs now | 10 updates/s, 16 makers |
|---|---:|---:|---:|
| Ed25519 | 10,836 B | 1.0x | 0.19 MB/s |
| ML-DSA-44 | 159,264 B | **14.7x** | 2.83 MB/s |
| ML-DSA-65 | 215,271 B | 19.9x | 3.83 MB/s |
| SLH-DSA-128s | 501,732 B | 46.3x | 8.92 MB/s |

**Fifteen times the bytes, and under three megabytes a second at rates a real
market maker uses.**

**So most of the stack goes post-quantum for a size penalty, and the quote proof
needs a construction this repository does not have** --- which is the same
construction section 5 wanted. Whichever reason takes you to lattices, the work
is the same work: **an assembly method that does not need the response to be
linear in the witness.**

---

## 7. What a quantum network would change, which is one thing

**It closes the one place this system is still computational.** The MPC is
unconditional; the traffic between the nodes runs over TLS. An eavesdropper who
records every link today and breaks TLS later holds every share and
reconstructs. **The computation is unconditional and the pipe carrying it is
not.**

The deployment shape is the one QKD can serve: seven fixed, long-lived, known
endpoints --- and `DEPLOYMENT.md` section 0 already puts them in one metropolitan
area for latency, which is the range at which QKD works.

**It cannot one-time-pad the traffic.** Against measured volumes:

| | traffic a quote | key rate for a one-time pad |
|---|---:|---:|
| default field, 0.4 quotes/s | 19.4 MB | **62 Mbps** |
| default field, 3 quotes/s | 19.4 MB | 465 Mbps |
| matched field, 0.4 quotes/s | 277.3 MB | 887 Mbps |

Metro-scale QKD produces roughly 0.1 to 10 Mbps --- literature, not measured
here --- so even the cheapest row is one to two orders short. A deployment would
rekey a symmetric cipher instead, **and symmetric ciphers are already
quantum-safe**: Grover halves the effective key strength and AES-256 absorbs it.
**The real gain is narrower than it sounds: not having to trust the key-exchange
assumption.** Against store-now-decrypt-later that is still a genuine defence.

**Where it cannot help, and not for engineering reasons.** Unconditionally secure
bit commitment is **impossible even with quantum mechanics** (Mayers; Lo--Chau,
1997). Quantum does not rescue the commitments, so section 6 is not avoidable by
building a quantum network instead.

The reason generalises: **a proof has to convince someone who was not on the
link** --- an auditor, a supervisor, a counterparty checking years later. QKD
secures a channel between two parties who are both present. Quantum digital
signatures exist but need quantum memory or repeated distribution and their
transferability is limited, so they do not give "anyone can verify this
afterwards" either.

| | does a quantum network change it |
|---|---|
| the MPC layer's security | no --- already unconditional |
| **confidentiality of inter-node traffic** | **yes --- this is the one** |
| commitments | no, by a no-go theorem rather than by engineering |
| the quote proof | no --- it must convince someone who was not there |
| signatures | no --- they must be transferable and checkable later |

*Quantum secret sharing* would distribute shares over quantum channels with
eavesdropping detection, but the shares here are classical, so the benefit
collapses into the row above. *Anonymous transmission* through entanglement
would hide which participant sent a request --- the transport layer's job,
approximated today by batching and shuffling on a fixed schedule --- and is
theoretically stronger and practically remote.

**A quantum network makes the pipe unconditional, not the proof.**

---

## 8. What the matched field leaves cross-region

**Almost exactly what the default field leaves**, because the round count does
not move and a wide-area quote is round-trip bound. Measured at 120 ms one way,
verified:

| | rounds | global | wall | vs default |
|---|---:|---:|---:|---:|
| default field, one request | 64 | 19.37 MB | 27.603 s | --- |
| **group order, one request** | **64** | **38.73 MB** | **29.179 s** | **1.06x** |
| default field, batch of 32 | 259 | 637.1 MB | 105.96 s | --- |

**Six per cent.** The earlier version of this section reported the matched field
at 0.036 quotes a second against 0.75 for the default --- a twenty-fold gap that
was the compile flag, not the field.

What survives from that work is the shape of batching, which is a property of
the circuit rather than of the field: **rounds grow with the batch** --- 64 to
259 from one request to thirty-two --- so batching trades the quote's age for
throughput. A batch of 32 is 3.3 s a quote against 27.6 s, eight times better,
for an age of 106 s against 28 s. Against `staleness.json`, where 96 seconds is
1.34 to 1.75 times the within-block floor, that is a good trade, and **batches of
8 to 32 are where throughput and age meet.**

## 9. Every prediction in this document that missed

Kept together rather than scattered, because a document that shows only the
predictions that landed is advertising a discipline rather than reporting one.

| predicted | measured | direction |
|---|---|---|
| matched field leaves the round count unchanged | **1.00x** | **landed** --- and was then reported as 2.02x for a week because of the compile flag |
| matched field costs about 2x traffic, from 16-byte elements becoming 32 | **2.00x** | **landed exactly**, and was likewise reported as 14.3x |
| edaBits reduce it | 150x worse --- preprocessing measured in a single-phase harness | wrong phase, **and against a problem that did not exist** |
| probabilistic truncation gives 150 to 250 MB | no change at all | wrong, same |
| the input check is free: one round and one field element | true of the opening; the mask was not counted | incomplete |
| the check therefore cannot run at 127 bits | it runs --- the first split of the budget that failed was taken for the whole budget | wrong |
| 2^-40 hiding is unreachable at 127 bits; then reachable; then unreachable | unreachable, ceiling about 2^-34 | resolved by measuring |
| 177-bit field: 4--9x traffic, 96--129 rounds, 1.4--1.9x wall | 7.8x, 124, 1.71x | landed, **against the wrong baseline** |
| the check adds 1 to 3 rounds | **1** | landed |
| cross-region at a batch of 32: 347 rounds, 0.38 quotes/s | 1,314 rounds, 0.036 --- extrapolating a ratio that was itself growing | 10x too generous, **and the ratio was an artifact** |
| compiling with `-F` instead of `-P` gives 2--6x traffic and 1.0--1.4x rounds | **2.00x and 1.00x** | landed |
| VOLE-in-the-Head co-paths cost `repeats*(depth*16 + 32)` = 2,560 B | **2,560 B** | landed exactly |
| VOLE-in-the-Head proof is 5--7 kB at 167 inputs | **45,616 B** | 7x too small |
| it proves in 8--20 ms and verifies in 8--20 | **73.06 and 69.94 ms** | 4x too fast |
| **the transform lands within 0.5--1.5x of Pedersen** | **3.93x and 4.52x** | **direction right, number wrong** |
| its proof is within 1.5x of Pedersen's | 8.39x | wrong |
| the one-time property is a real constraint and not a detail | it is; `OneTimeError` | landed |

**All five VOLE-in-the-Head misses have one cause.** The construction was costed
as though the witness were bits, which is FAEST's setting and the setting every
published number for it is in. It is not this stack's setting. Over a 127-bit
prime every term that FAEST pays in bits --- the consistency corrections, the
PRG output per leaf --- is paid in 16-byte elements, and the arithmetic was
written down without that substitution. **The prediction that mattered was still
right in direction**: the 113x advantage does not survive the transform. It was
wrong about which side of one the answer lands on.

**The two most important predictions in this document were right the first
time**, and were then buried under a measurement that contradicted them for a
week. When arithmetic said the cost should be the element width and the
measurement said fourteen times, the arithmetic was correct and the harness was
misconfigured --- and the tool was printing the reason on every run.

That is the finding to carry out of here. The other errors were counting
mistakes, which are cheap to find once someone counts again --- and the
VOLE-in-the-Head row above is a third kind, which is taking a published cost
model and not checking which field it was written for. **This one was a
disagreement between theory and measurement that got resolved in favour of the
measurement without asking why they disagreed**, and it cost four sections and a
published slide.
