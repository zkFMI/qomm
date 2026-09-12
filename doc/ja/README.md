# Japanese documentation and slide decks

## Source-code documentation

| file | purpose |
|---|---|
| [`SOURCE_CODE_GUIDE.md`](SOURCE_CODE_GUIDE.md) | QOMM・DeFMI・zkPIの実行経路、秘密境界、状態遷移、障害時動作を端から端まで解説 |
| [`MPC_ZKPI_DEFMI_FLOW.md`](MPC_ZKPI_DEFMI_FLOW.md) | Maker/Taker事前承認、保証枠、同時RFQ、共同zkPI、口座を名指ししないDeFMI/AvalancheのDvPを、実装済みと外部未了に分けて解説 |
| [`SOURCE_FILE_INDEX.md`](SOURCE_FILE_INDEX.md) | 2026-08-29時点の第一者ソースをファイル単位で列挙し、責務・試験・変更時の影響先を対応づける |
| [`DEFMI_ZKPI_USE_CASES.md`](DEFMI_ZKPI_USE_CASES.md) | QOMMを前提にせず、DeFMIとzkPIをファンド、担保、国際決済、証券、登記などへ使う方法、既存実証との差、実装順を整理 |
| [`BOJ_JGB_COLLATERAL_SETTLEMENT.md`](BOJ_JGB_COLLATERAL_SETTLEMENT.md) | 日銀型の国債DVP、法人単位共通担保、日中当座貸越、同時RFQ、Avalanche正本状態、クロスDeFMIとの差を一次資料と実装に対応づけて解説 |
| [`AETHEL_STREAMING_RECEIVABLE_L1.md`](AETHEL_STREAMING_RECEIVABLE_L1.md) | Aethelのストリーム債権、審査・保証・前払資金providerの分離、zkPI binding、DeFMI note/DvP、Avalanche L1状態を実装に対応づけて解説 |

## Slide decks

Updated 2026-09-12: current implementation boundaries, note-proof corrections,
PQC research status and public repository links. Historical measurements retain
their original environments; the revision date does not imply new measurements.

Three decks over the same design system. They are in Japanese; the rest of this
repository is in English, and this directory is where that is declared rather
than an exception somebody has to remember.

| file | pages | for |
|---|---:|---|
| `qomm_intro.pdf` | 22 | no prerequisites, no formulas |
| `qomm_slides.pdf` | 79 | readers who already know the field |
| `qomm_tech.pdf` | 48 | construction, proofs and measurements |

Built from `papers/qomm/slides/*.tex` in the private working tree, which is not
published. `make` there writes the PDFs here as part of building them, so the
published copies cannot fall behind the source that produced them.

Historical measurements come from `artifacts/`. September additions also cite
versioned acceptance and research receipts. The private source tree records exact
source hashes and PDF verification in `papers/qomm/slides/UPDATE_2026-09-12.md`.
PQC research execution is explicitly distinguished from full operational adoption.
