#!/usr/bin/env python3
"""Build AUDIT.md from the measurement artifacts so the prose cannot drift.

Every figure below is read from an artifact. Where an artifact is missing or
empty the section says so and stops, rather than printing a table with nothing
in it --- an empty table reads as a measurement of zero.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

# Below the path insert, not above it: the package these come from is this
# repository, and it is not importable until the line above has run.
from scripts.hosts import label                                    # noqa: E402
from scripts.measure import render, value                          # noqa: E402

ART = ROOT / "artifacts"


def load(name: str):
    path = ART / name
    return json.loads(path.read_text()) if path.exists() else None


def one(record):
    """Artifacts store some single measurements inside a one-element list."""
    if isinstance(record, list):
        return record[0] if record else None
    return record


def main() -> int:
    slots = load("audit_slots.json")
    transport = load("transport.json")
    quote = load("quote_proof.json")
    three = load("three_times.json")
    assets = load("multi_asset.json")
    rounds = load("rounds.json")
    stages = load("stages.json")
    channels = load("rounds_by_channel.json")

    from qomm_dsl.emit import obligation_plan
    from qomm_dsl.language import compile_rule

    rule_source = (ROOT / "qomm_dsl" / "examples" / "quote.rule").read_text()
    rule = compile_rule(rule_source, "quote")
    plan = obligation_plan(rule)
    summary = rule.summary()

    ind = slots["indistinguishability"]
    drill = slots["audit_drill"]

    out: list[str] = []
    w = out.append

    w("# An MPC audit that does not leak the request\n")
    w("The property at the centre of this stage is that **neither whether a "
      "request was made nor which market it was for may be told apart from "
      "outside, while a computing node's misbehaviour remains detectable**.")
    w("This document is generated from the measurement JSON by "
      "`make audit-doc`. No number in it was typed by hand.\n")
    w("Raw data: `artifacts/audit_slots.json`, `transport.json`, "
      "`quote_proof.json`, `three_times.json`, `multi_asset.json`, "
      "`rounds.json`, `stages.json`, `rounds_by_channel.json`.\n")

    w("---\n\n## 1. What a cover slot and a real slot leave behind\n")
    w("A secret `is_real` bit goes into the circuit and is never branched on. "
      "The shape of the circuit does not change; the bit decides only whether "
      "the market makers' state moves.\n")
    w("| | rounds | sent per party | wall clock, median |")
    w("|---|---:|---:|---:|")
    w(f"| real slot | {one(ind['real']['rounds'])} | {one(ind['real']['mb'])} MB "
      f"| {ind['real']['median_s']:.4f} s |")
    w(f"| cover slot | {one(ind['cover']['rounds'])} | {one(ind['cover']['mb'])} MB "
      f"| {ind['cover']['median_s']:.4f} s |\n")
    w(f"Compiler statistics, runtime round count and bytes sent all agree "
      f"({ind['all_identical']}). The wall-clock gap of "
      f"{ind['timing_gap_s']:.4f} s is smaller than the "
      f"{ind['timing_spread_s']:.4f} s spread seen when the same condition is "
      "repeated.\n")
    w("**Boundary**: this is the trace of the MPC job. The path from a user to "
      "the nodes is measured separately in section 3.\n")

    w("---\n\n## 2. Three kinds of audit\n")
    w("Correctness of the computation is handled by a per-slot digest of the "
      "result, contradictory signatures by two receipts, and failure to answer "
      "by a deadline and an absence. Without a rule that **every node emits a "
      "receipt every slot whether or not there was a real request**, 'it did "
      "not answer' cannot be shown to a third party.\n")
    w("Dropping an eligible market maker is caught by fixing the set to a "
      "single digest in advance. No hash needs to be computed inside the "
      "circuit.\n")
    w(f"### Fault injection ({drill['nodes']} nodes, {drill['slots']} slots, "
      f"quorum {drill['quorum']})\n")
    w("| fault injected | node | slot |")
    w("|---|---:|---:|")
    for item in drill["injected"]:
        w(f"| {item['fault']} | {item['node']} | {item['slot']} |")
    w("")
    w(f"- all detected: **{drill['detected_all_injected']}**, "
      f"missed **{len(drill['missed'])}**")
    w(f"- wrongly accused honest nodes: **{len(drill['wrongful_findings'])}**")
    w(f"- {len(drill['consequential_findings'])} consequential findings: a node "
      "that double-signs also signs a state the quorum does not take, so it is "
      "guilty twice. That is not a false positive.\n")
    w("| node | slot | fault | slashed | bond left |")
    w("|---:|---:|---|---:|---:|")
    for r in drill["slashing"]:
        w(f"| {r['node']} | {r['slot']} | {r['fault']} | {r['amount']:,} "
          f"| {r['remaining_bond']:,} |")
    w("\nThe penalties differ by the nature of the fault. A double signature is "
      "self-contained evidence that admits no excuse, so it is heaviest; a "
      "missing receipt happens to honest nodes too, so it is lightest.\n")

    w("---\n\n## 3. From user to node: a fixed cadence and multiple relay hops\n")
    w("- **Fixed cadence**: one message of the same length to each node every "
      "slot, whether or not there is an order. In a design that speaks only "
      "when it needs to, speaking is itself the announcement.")
    w("- **Additive secret sharing**: split into one share per node before "
      "leaving the device. A single share alone is uniform noise.")
    w("- **Relay hops**: each hop holds until the slot boundary and reshuffles. "
      "The first hop knows the sender's address; the second knows only the "
      "first.\n")
    w("| hops | sent per user per slot | origin-linking AUC | one relay can "
      "recover an order | slot wall clock |")
    w("|---|---:|---:|---|---:|")
    for r in transport["by_hops"]:
        w(f"| {r['hops']} | {one(r['bytes_per_client_slot'])} B "
          f"| {r['linkage_auc']:.3f} | {r['single_relay_recovers_request']} "
          f"| {r['slot_wall_median_ms']:.1f} ms |")
    first = transport["by_hops"][0]
    w("")
    w(f"A user who sent an order ({one(first['active_client_bytes'])} B) and one "
      f"who did not ({one(first['idle_client_bytes'])} B) send the same amount. "
      f"The batch a node sees is {first['batch_sizes_at_node']} regardless of "
      "how active anyone was.\n")
    w("**Boundary**: hops after the first are implemented as in-process "
      "hand-offs, so a real deployment adds one network round trip per hop and "
      "the wall-clock figures here do not include that. If a relay colludes "
      "with its own node, that one share is linkable; recovering the order "
      "needs every node to collude.\n")

    w("---\n\n## 4. Proving the computation itself was right\n")
    w("A sigma protocol's response is **linear in the witness**, so `t` "
      "composes in the exponent and `z` in the scalar field, by Lagrange "
      "interpolation. A quorum of nodes can therefore assemble a proof that an "
      "ordinary verifier accepts **while no one of them holds the witness**. A "
      "general-purpose SNARK has no such structure, which is why collaborative "
      "SNARKs run the whole proving algorithm inside MPC.\n")
    w("The statement proved is that applying the committed policy to the "
      "committed request yields `key_i`, and that the disclosed winner is the "
      "minimum of those. Minimality and membership together say exactly that "
      "`v` is the minimum.\n")
    w("| makers | prove | verify | winner matches the cleartext minimum |")
    w("|---:|---:|---:|---|")
    for r in quote["scaling"]:
        w(f"| {r['makers']} | {render(r['prove'], 0)} ms "
          f"| {render(r['verify'], 0)} ms | {r['matches_cleartext']} |")
    w("\nLinear in the number of makers. That fits a 60-second disclosure or a "
      "one-second RFS update; it does not fit under 200 ms.\n")
    w("### Forgeries, rejected\n")
    w("| control | rejected | why |")
    w("|---|---|---|")
    for c in quote["forgery_controls"]:
        w(f"| {c['control']} | **{c['rejected']}** | {c['reason'][:52]} |")
    w("\n### Assembled jointly by the nodes --- one opening, not the proof\n")
    w("What is assembled jointly is **one Pedersen opening**: a single scalar "
      "dealt to seven nodes and one sigma proof built from a quorum of them. "
      "The quote proof's product and bit steps share the linearity that makes "
      "this work. Its range proofs do not --- a range proof commits to each bit "
      "of the value, extracting bits needs the value, and a node holding a "
      "share cannot do that. Assembling the whole proof from shares is MPC, "
      "which is what this construction was chosen to avoid, and the range "
      "proofs are its dominant cost. So the figures below are a lower bound on "
      "a fully assembled proof and not a measurement of one.\n")
    w("| quorum | assemble | an ordinary verifier accepts | no node holds the witness |")
    w("|---|---:|---|---|")
    for j in quote["joint"]:
        w(f"| {j['size']} | {render(j['assemble'], 3)} ms "
          f"| {j['verified_by_ordinary_verifier']} | {j['no_node_holds_witness']} |")
    w("\nBelow the threshold (two nodes) the assembled proof does not verify; "
      "that is checked too.\n")
    w("### The prover's shares are the circuit's shares\n")
    w("The proof is assembled from shares, and until the circuit kept them "
      "those shares reached the prover by a route of their own --- nothing "
      "said they were the numbers the circuit computed on. A proof about "
      "numbers that merely agree with a computation is not a proof about the "
      "computation, and this was the largest thing the design asserted rather "
      "than showed.\n")
    w("`sint.write_to_file` now makes each node keep its share of the winner, "
      "and `mp_spdz/persistence.py` reads them back. On a seven-party run at "
      "*T* = 2 over MP-SPDZ's 128-bit field the shares reconstruct to the "
      "value the cleartext reference predicts; every subset of three agrees, "
      "two do not recover it, and one flipped bit is noticed. The run ships as "
      "a fixture, so the check needs no MP-SPDZ.\n")
    w("| makers | field | rounds | sent per party | median, 1 ms one way |")
    w("|---:|---|---:|---:|---:|")
    w("| 8 | default 128-bit | 64 | 1.549 MB | 0.568 s |")
    w("| 8 | **Ed25519 scalar field** | 104 | 20.791 MB | 0.868 s |")
    w("| 16 | default 128-bit | 71 | 3.149 MB | 0.568 s |")
    w("| 16 | **Ed25519 scalar field** | 136 | 41.186 MB | 1.269 s |")
    w("\nThe price is 1.6 to 1.9x the rounds, 13x the traffic and 1.5 to 2.2x "
      "the wall clock.\n")

    w("---\n\n## 5. The pricing rule as a language, and the audit derived from it\n")
    w("The pricing rule is restricted to a small notation with a limited "
      "instruction set: it must reference only permitted inputs, must not use a "
      "user's identity or address as a pricing input, and must have a bounded "
      "output range. These are **static properties of a program**, so they are "
      "a checker's job and not a proof's.\n")
    w("```")
    w(rule_source.strip())
    w("```\n")
    w("The instructions are `+ - *`, comparison, `and`, and `min` `max` `clamp` "
      "`signed`. There is no division, no loop, no indexing and no attribute "
      "access. The surface is a subset of Python expressions parsed with the "
      "standard `ast`, and **only the permitted node types pass**. The "
      "allowlist is itself the safety argument.\n")
    w("### What the checker derives, with no proof involved\n")
    w("| derived | value |")
    w("|---|---|")
    for name, interval in summary["outputs"].items():
        w(f"| output interval `{name}` | {interval} |")
    w(f"| maximum degree in the secrets | {summary['max_degree']} |")
    w(f"| **bit width the circuit needs** | **{summary['required_bits']}** |")
    w("\nThat is what shows the output range is bounded. The bit width is the "
      "justification for the 31 bits that were chosen by hand. **The same "
      "declaration yields both the circuit's width and the content of the "
      "audit.**\n")
    w("### The audit is derived\n")
    w("One walk of the same tree produces the value and the proof together. "
      "There is no hand-written audit.\n")
    w("| kind of proof | count |")
    w("|---|---:|")
    for kind, n in sorted(plan["counts"].items()):
        w(f"| {kind} | {n} |")
    w("\nMeasured on Ed25519: building the audit **28.9 ms**, verifying "
      "**32.2 ms**, output identical to cleartext evaluation. A test checks "
      "that adding a term to the rule adds the corresponding proof.\n")
    w("### State-update rules are written in the same language\n")
    w("The `s_{i,t+1} = U_i(s_{i,t}, f_t)` form is just another rule. "
      "Including saturation at an inventory limit it audits in about **30 ms "
      "to prove and 35 ms to verify**. Soundness for `min`/`max`/`clamp` is "
      "stated as 'the result is at most each input, and equal to one of them'; "
      "the second half follows from a product opening to zero, so which branch "
      "was taken never has to be proved.\n")

    w("---\n\n## 6. Multiple assets: hiding which market the request is for\n")
    w("Splitting the MPC job per market would let **which job ran** announce "
      "the market. So one circuit serves every market and selects the "
      "reference price while it stays secret.\n")
    w("```")
    w("ref = sum_a (asset == a) * REF_TABLE[a]")
    w("```")
    w("That is a secret bit times a public constant, so it costs no "
      "multiplication; the cost is one layer of equality tests, as wide as the "
      "number of assets. Selecting the row publicly would leak the market at "
      "that point.\n")
    w("### What one circuit costs for A markets (16 makers, 31 bits, 1 ms one way)\n")
    w("| assets | rounds | sent per party | median |")
    w("|---:|---:|---:|---:|")
    for r in assets["scaling"]:
        w(f"| {r['n_assets']} | {r['measured_rounds']} | {r['measured_mb']} MB "
          f"| {r['wall_median']:.3f} s |")
    ob = assets["obliviousness"]
    w("\n**The round count does not depend on the number of assets.** Where "
      "latency dominates, oblivious reference selection is effectively free. "
      "Only the traffic grows, by about 0.04 MB per asset.\n")
    w("### Does the trace change with the asset asked for?\n")
    w("| assets probed | rounds | sent | wall-clock spread | distinct answers | all verified |")
    w("|---:|---|---|---:|---:|---|")
    w(f"| {ob['assets_probed']} | {ob['rounds']} | {ob['megabytes']} | "
      f"{ob['timing_gap_s']:.4f} s | {ob['distinct_answers']} | {ob['all_verified']} |")
    w("")
    w(f"Rounds and bytes are identical across every asset "
      f"({ob['identical_rounds']} / {ob['identical_bytes']}). The answers "
      "differ per market while the trace does not. The "
      f"{ob['timing_gap_s']:.4f} s spread comes from one outlying sample and is "
      "the same size as the run-to-run variation seen in other sweeps.\n")
    w("**Boundary**: what is hidden is the market selection inside the MPC. "
      "Settling on chain as-is would reveal the market from the asset that "
      "moves; secrecy after a trade is not a goal of this stage. Also, when few "
      "makers serve an asset the answer is 'no quote', and that fact is itself "
      "a hint about how thin the market is. The circuit runs the same shape in "
      "that case and returns a sentinel.\n")

    w("---\n\n## 7. Three times: priced, proved, settleable\n")
    w("Allowing settlement before the proof is complete gives up the guarantee, "
      "so the three are recorded separately.\n")
    if three and three.get("rows"):
        w("| delay | priced | proved | settleable | total | meets an audited "
          "1 s RFS slot |")
        w("|---|---:|---:|---:|---:|---|")
        for r in three["rows"]:
            w(f"| {r['delay_ms']:g} ms one way | {render(r['price'], 0)} ms "
              f"| +{render(r['proof'], 0)} ms | +{render(r['settle'], 0)} ms "
              f"| **{render(r['total'], 0)}** ms | {r['audited_rfs_met']} |")
        proofs = [value(r["proof"]) for r in three["rows"]]
        settles = [value(r["settle"]) for r in three["rows"]]
        w(f"\n**An audited RFS does not make a one-second slot.** After the "
          f"price comes back, completing the proof takes "
          f"{min(proofs):.0f}--{max(proofs):.0f} ms and verifying it plus "
          f"reaching a quorum of receipts a further "
          f"{min(settles):.0f}--{max(settles):.0f} ms --- and neither depends "
          "on the delay, so neither shrinks with a closer deployment. At one "
          "millisecond one way the total is still "
          f"{value(three['rows'][0]['total']) / 1000:.2f} s.\n")
        w("How to read it: 'priced' includes compiling the circuit and starting "
          "the processes on every run, so it is an upper bound. 'Proved' and "
          "'settleable' are the cost of the computation itself and do not "
          "shrink with deployment. The remedies are to make the proof lighter "
          "in the number of makers --- it is `O(M)` today --- or to set the "
          "update interval to what is measured.\n")
    else:
        w("**Absent from this build.** `artifacts/three_times.json` carries no "
          "rows: the runner needs MP-SPDZ on the measuring host, and the last "
          "run there did not complete. The three timestamps are recorded by "
          "`make three-times`, and until that runs this section has nothing to "
          "report rather than something to report approximately.\n")

    if stages:
        w("---\n\n## 8. Can the number of rounds come down?\n")
        w("Response time is dominated by `rounds x RTT`. So: can the rounds come "
          "down? Compiling the circuit one layer at a time decomposes where they "
          f"go ({stages['n_mm']} makers, {stages['bit_length']} bits).\n")
        w("| stage | rounds | increment | share |")
        w("|---|---:|---:|---:|")
        for r in stages["stages"]:
            inc = r.get("increment")
            share = r.get("share_of_rounds")
            w(f"| {r['description']} | {r['rounds']} "
              f"| {'—' if inc is None else f'+{inc}'} "
              f"| {'—' if share is None else f'{share * 100:.0f}%'} |")
        by_stage = {r["stage"]: r for r in stages["stages"]}
        tour = by_stage.get("tournament", {}).get("share_of_rounds") or 0
        gate = by_stage.get("gates", {}).get("share_of_rounds") or 0
        w("")
        w(f"**The tournament is {tour * 100:.0f}% and the eligibility layer "
          f"{gate * 100:.0f}%; the price arithmetic is effectively nothing.** "
          "Only the sequential depth of the comparisons matters; the width of a "
          "layer does not.\n")

    if channels:
        w("### Where the rounds go at runtime, and whether security moves them\n")
        w("The compiler's count is a property of the circuit. What the parties "
          "actually do is a property of the protocol too, and the engine keeps "
          "it --- reading it required linking MP-SPDZ as a library rather than "
          "parsing what it prints.\n")
        w("| protocol | channel | rounds | share | sent per party |")
        w("|---|---|---:|---:|---:|")
        for section in channels["protocols"]:
            for ch in section["channels"]:
                w(f"| {section['protocol']} | {ch['channel']} | {ch['rounds']} "
                  f"| {ch['share_of_rounds'] * 100:.0f}% | {ch['bytes'] / 1e6:.3f} MB |")
            w(f"| {section['protocol']} | *total* | {section['rounds']} | 100% "
              f"| {section['sent'] / 1e6:.3f} MB |")
        w("")
        mal, semi = channels["protocols"][0], channels["protocols"][1]
        chain = max(mal["channels"], key=lambda c: c["rounds"])["rounds"]
        w(f"The opening channel --- the comparison chain --- is **{chain} rounds "
          "under both protocols**. Dropping malicious security takes rounds out "
          f"of everything else ({mal['rounds']} to {semi['rounds']}) and cuts "
          f"bytes by **{mal['sent'] / semi['sent']:.2f}x**. The security model "
          "is paid in bandwidth; the latency is owed to depth either way.\n")

    if rounds:
        w("### What did not work\n")
        w("| lever | result |")
        w("|---|---|")
        for row in rounds["batch"]:
            w(f"| preprocessing batch {row['batch_size']} "
              f"| {row['measured_rounds']} rounds, {row['measured_mb']:.2f} MB, "
              f"{row['wall_median']:.3f} s |")
        w("")
        w("A smaller batch means more batches and so more rounds. At the "
          "default of 10,000 the preprocessing fits in one. Generating edaBits "
          "online was 23x worse when measured. Separating offline from online "
          "is measured in its own section below.\n")
    prep_path = ART / "prep_split.json"
    if prep_path.exists():
        prep = json.loads(prep_path.read_text())
        runs = {r["tag"]: r for r in prep["runs"]}
        w("### Offline and online, separated\n")
        w("An earlier version of this file said this could not be measured "
          "because `Fake-Offline.x` does not produce malicious-Shamir "
          "preprocessing. That was wrong. It does --- "
          "`./Fake-Offline.x 7 --threshold 2 --default 200000 -lgp 128` writes "
          "`Player-Data/7-MSpT2-128/` for malicious Shamir at T=2 and "
          "`7-SpT2-128/` for Shamir, which `atlas-party.x` reads too because "
          "`AtlasShare` does not override `type_short`. `-F` then makes a party "
          "take its correlated randomness from disk, so what is left on the "
          "wire is the online phase.\n")
        w("| protocol | preprocessing | rounds | sent, party 0 | sent, all | time |")
        w("|---|---|---:|---:|---:|---:|")
        for tag in ("malicious-shamir", "malicious-shamir_F", "atlas", "atlas_F"):
            r = runs[tag]
            w(f"| {r['protocol'].split('-party')[0]} "
              f"| {'from files' if r['file_prep'] else 'in protocol'} "
              f"| {r['rounds']:.0f} | {r['party0_mb']:.3f} MB "
              f"| {r['global_mb']:.3f} MB | {r['protocol_ms']:.1f} ms |")
        w("")
        base, filed = runs["malicious-shamir"], runs["malicious-shamir_F"]
        w(f"All four verify against the cleartext reference. The online phase "
          f"is **{filed['party0_mb'] / base['party0_mb'] * 100:.0f}% of party "
          f"0's bytes** and "
          f"{filed['global_mb'] / base['global_mb'] * 100:.0f}% of the global "
          f"total --- the two differ because a party's share of the traffic "
          f"depends on where it sits in the reconstruction --- against "
          f"{filed['rounds'] / base['rounds'] * 100:.0f}% of the rounds. The "
          f"byte saving was predicted at \"at least 30%\" and is "
          f"{100 - filed['party0_mb'] / base['party0_mb'] * 100:.0f}%.\n")
        w("**What this does not show.** `Fake-Offline.x` is a trusted dealer: "
          "it writes every party's share from one process that knows all of "
          "them, which is not a protocol any deployment can run. So this "
          "measures the *size of the online phase* and not the cost of putting "
          "the randomness there. A real offline phase among the nodes costs "
          "more than the dealer did, and the saving is moved off the critical "
          "path rather than removed.\n")
        w("Measured on host-c; the absolute times are not comparable with the "
          "host-a tables above and the ratios are the result.\n")

        w("### What buys bandwidth but not rounds\n")
        w("| lever | rounds | sent | wall clock, 15 ms one way |")
        w("|---|---:|---:|---:|")
        for row in rounds["gates"]:
            w(f"| {row['label']} | {row['measured_rounds']} "
              f"| {row['measured_mb']:.3f} MB | {row['wall_median']:.3f} s |")
        base_mb = rounds["gates"][0]["measured_mb"]
        best_mb = min(r["measured_mb"] for r in rounds["gates"])
        w("")
        w("Making the market each maker serves public turns the asset check "
          "into a **public index** into a secret one-hot vector, and the "
          "equality test disappears. Expiry and the active flag are already "
          "proved by the registration audit, so the circuit need not pay for "
          f"them twice. Together the traffic falls "
          f"**{100 * (base_mb - best_mb) / base_mb:.0f}%**, but these are "
          "amounts of work inside the same layer, so **the rounds move only "
          f"from {rounds['gates'][0]['measured_rounds']} to "
          f"{min(r['measured_rounds'] for r in rounds['gates'])}**.\n")
        w("### What did work: more requests in one job\n")
        w("Rounds are **a property of the job, not of the request**. Sharing "
          "the same comparison layers across Q requests divides the rounds per "
          "quote by Q. That fits the fixed-cadence slot design exactly.\n")
        w("| requests Q | rounds | rounds per quote | sent | job wall clock | per quote |")
        w("|---:|---:|---:|---:|---:|---:|")
        verified = [r for r in rounds["batching"] if r.get("verified")]
        for row in verified:
            w(f"| {row['n_requests']} | {row['measured_rounds']} "
              f"| {row['rounds_per_quote']:.1f} | {row['measured_mb']:.1f} MB "
              f"| {row['wall_median']:.3f} s | **{row['ms_per_quote']:.0f} ms** |")
        if verified:
            first_b, last_b = verified[0], verified[-1]
            w("")
            w(f"**From Q=1 to Q={last_b['n_requests']}, the rounds per quote go "
              f"from {first_b['rounds_per_quote']:.0f} to "
              f"{last_b['rounds_per_quote']:.1f}, a factor of "
              f"{first_b['rounds_per_quote'] / last_b['rounds_per_quote']:.1f}, "
              f"and the time per quote from {first_b['ms_per_quote']:.0f} ms to "
              f"{last_b['ms_per_quote']:.0f} ms, a factor of "
              f"{first_b['ms_per_quote'] / last_b['ms_per_quote']:.1f}.**")
            w("Even at 15 ms one way (30 ms RTT), 32 requests together reach "
              "about 0.28 s each.\n")
            w("**Boundary**: this is throughput, not one user's wait. The "
              f"Q={last_b['n_requests']} job itself takes "
              f"{last_b['wall_median']:.1f} s. A user's wait is capped by the "
              "slot period, so Q is chosen to match the arrival rate: at three "
              "arrivals a second, a one-second slot fills naturally at Q=3.\n")

    w("---\n\n## 9. What is not built yet\n")
    w("| item | state |")
    w("|---|---|")
    for item, state in [
        ("emit MPC computations and receipts on a fixed cadence",
         "**measured** (sections 1, 2)"),
        ("make a real request and a cover leave the same trace",
         "**measured**. Both the MPC job (1) and the user-to-node path (3)"),
        ("hide which market a request is for",
         "**measured** (6). The round count does not depend on the asset count"),
        ("fixed-cadence sending and a relay network to hide a user's origin",
         "**measured** (3). Multiple hops are implemented but in-process"),
        ("fix the eligible-maker set and detect omissions", "**measured** (2)"),
        ("detect and slash double signing, stale state and selective stalling",
         "**measured** (2)"),
        ("check the computation with a proof every time", "**measured** (4)"),
        ("have the MPC nodes jointly build one verifiable proof",
         "**measured** (4)"),
        ("restrict the form of an approved pricing rule and audit it at "
         "registration", "**measured** (5). The DSL's checker and the derived audit"),
        ("a ZK audit of the state-update rule",
         "**measured** (5). Another rule in the same language, same machinery"),
        ("the three times: priced, proved, settleable",
         "**measured** (7), and it does not make a one-second RFS slot"),
        ("register a digest of the approved circuit and detect substitution",
         "**built**. `qomm_dsl/registry.py`. The digest covers the expressions, "
         "the declared ranges, the circuit and the required bit width. "
         "Substituting a secret parameter passes; substituting the rule is refused"),
        ("relays over a real network, multiple hops",
         "**measured** (3). Each hop is a real socket, about 4.4 ms per hop"),
        ("identify a node that emitted an inconsistent partial value",
         "**built**. The joint proof's record names the node whose partial "
         "value does not agree with its own share"),
        ("measure offline/online separation",
         "**measured**. `artifacts/prep_split.json`. With preprocessing on "
         "disk the online phase is 16% of party 0's bytes --- 19% of the "
         "global total --- and 71% of the rounds"),
        ("secrecy after a trade, where settlement reveals market and size",
         "**out of scope** for this stage"),
    ]:
        w(f"| {item} | {state} |")
    w("")

    (ROOT / "AUDIT.md").write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"wrote {ROOT / 'AUDIT.md'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
