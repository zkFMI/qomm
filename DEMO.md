# Running the demo

    cargo run -j 4 --release --manifest-path rust/Cargo.toml -p qomm-harness --bin serve_demo

Open the address it prints. Cargo builds the Rust workspace on the first run,
and the server listens on every interface so a
second machine can reach it by typing the address rather than by being the same
machine.

## The point of it

The claim this project makes is about who knows what, and one screen showing
everything is the one presentation that cannot make it. So the demo is seats. A
browser holds one role and is sent what that role would see, and nothing else
reaches it --- opening the developer console shows the same thing a person
without one is shown.

    taker       sends the order. It is split before it leaves the page, and no
                node is handed the whole of it. This seat alone gets a price.
    maker       leaves a price policy. It never sees the order, and does not
                learn that it won until the taker tells it.
    node        computes. It holds shares and nothing else. It can also cheat,
                and what happens then splits three ways.

Every seat nobody is in runs itself, so one laptop with nothing claimed shows a
working market immediately, and nine laptops with a person at each show the same
market with people in it. Hand a seat out by pasting its link:

    http://<host>:8800/?seat=node:3&label=Rin
    http://<host>:8800/?seat=maker:1&label=Ann
    http://<host>:8800/?seat=observer

The observer seat is the screen at the front of the room. It shows everything
and says on every frame that it is a view no deployment has.

The page follows the browser's language, and `?lang=en` or `?lang=ja` overrides
it.

## Three ways to cheat, three different endings

The node seat is where the demo earns its keep. The choices are not flavours of
one thing --- they are different rungs of `ACCOUNTABILITY.md`, and the whole
reason to sit a person at a node is to let them find that out by pressing.

| what the node does | what happens |
| --- | --- |
| a wrong share during a multiplication | corrected, the node named, **the protocol does not stop** |
| a third node does the same | beyond the capacity: the round stops and nobody is named |
| a wrong share at the final opening | decoded and named too, and more nodes can do it before it fails |
| a share other than the one it was dealt | nothing is inconsistent. With the commitment check off, **nobody notices and the price is simply wrong** |
| the same, commitment check on | refused before anything is computed, the node named |
| goes quiet after the inputs | one fewer evaluation point, so one fewer liar can be corrected |
| never takes part at all | the round cannot start: inputs are the sum of every node's share |

That table is not a script. `rust/qomm-demo/src/protocol.rs` deals real additive shares,
does a real Shamir multiplication in the Damgard--Nielsen shape --- local
product to degree `2t`, mask with a double sharing, open, subtract --- and
decodes the opening with `qomm_audit::locate`, which is Berlekamp--Welch. When
the screen says a node was named, an error locator polynomial said so.

The last two rows are the ones worth dwelling on. Robustness was built for the
degree reduction and for nothing else, and a demo that let a node vanish and
kept going would be claiming something this system does not have.

## What is real and what is not

There are two engines and the badge in the corner of every page says which one
produced the number on screen.

**`sim`, the default.** The share layer is real, as above. The tournament that
picks the winner is computed in the clear: comparing two secret prices needs bit
decomposition, and building that here would be building a second MPC engine
beside the one this project already measures. A round takes a couple of
milliseconds, so it runs on anything.

**`mpc`.** The whole circuit, comparisons included, compiled and executed by
MP-SPDZ. The price shown is the one that came out of `QOMM_MASKED_KEY` in a
party's log, and it is checked against the cleartext reference every round --- a
round that does not match is reported as not matching. What a node seat is shown
is its own party input file, because that is what it holds.

    cargo run -j 4 --release --manifest-path rust/Cargo.toml -p qomm-harness --bin serve_demo -- --engine mpc --nodes 7 --threshold 2 \
        --mp-spdz-root ~/work/qomm/MP-SPDZ

A round is a few hundred milliseconds against a couple, and it needs a built
MP-SPDZ with certificates for the party count in use.

**The misbehaviour switches reach it when the build can carry them.** With
`atlas-party.x` and the robust patch, a node seat set to send a wrong share
during a multiplication corrupts a **real party in a real protocol**: the others
correct it, the answer still verifies against the cleartext reference, and the
log names exactly who. Measured at nine parties and threshold two:

| node seats lying | price | verified | named | rounds |
|---|---:|:---:|---|---:|
| none | 15878 | yes | --- | 62 |
| node 0 | **15878** | yes | **[0]** | 62 |
| nodes 0 and 4 | **15878** | yes | **[0, 4]** | 62 |

The engine picks `atlas-party.x` when the tree has one and `n >= 4T+1`, and says
why not when it does not --- a build without the patch, or too few nodes for a
wrong share in a multiplication to be corrected rather than only detected. The
browser reads that reason rather than assuming, so a stopped switch says which
of the two it is.

The other switches --- a wrong share at the final opening, a substituted input,
going quiet --- still run in the demo's own share layer only, and the banner
says so.

The demo's market is the circuit's market, and that is a test rather than a
claim: `rust/qomm-demo/tests/model.rs` prices twenty-five random fixtures through both
`qomm_demo::model` and the generator's own cleartext reference and requires every
quote to agree.

## What each seat is shown, and what it is not

The round is computed once, in full, and then projected onto each connection.
A projection can only delete.

| | taker | maker | node | observer |
| --- | --- | --- | --- | --- |
| the order | its own | never | never | yes |
| a policy | never | its own | never | all |
| the mask | its own | never | never | yes |
| the price and the winner | yes | only if it won, and only when told | never | yes |
| the opened key | yes | yes | yes | yes |
| shares | never | never | its own | --- |
| a node's chosen behaviour | never | never | its own | all |
| who was named, and how many openings were corrected | yes | yes | yes | yes |

The opened key is `best_key + mask`: it packs the price and the winning maker
together and it is uniform to anyone without the taker's mask. So the makers do
not learn who won either, and somebody has to tell them. In the demo that is the
taker pressing a button, which is the honest shape of it and is worth watching
happen rather than being told about.

## Options worth knowing

    --nodes 9 --threshold 2     the design point. n >= 4T+1 is what lets a wrong
                                share in a multiplication be corrected rather
                                than only detected; at 7 and 2 it is detected
    --makers 8                  how many maker seats there are
    --round-seconds 8           how often the clock starts a round. The clock
                                stops while a person is in the taker seat ---
                                asking is the taker's job
    --step-ms 350               how long each phase is held on screen. Pacing for
                                the room, not protocol time, and the view reports
                                what the arithmetic took separately. 0 turns it off
    --no-auto-rounds            nothing happens until somebody presses send
    --no-input-check            start with the commitment check off, so a node
                                feeding a share it was not dealt goes unnoticed

The observer seat can change all of those while the demo is running.
