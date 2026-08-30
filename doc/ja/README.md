# Japanese documentation and slide decks

## Source-code documentation

| file | purpose |
|---|---|
| [`SOURCE_CODE_GUIDE.md`](SOURCE_CODE_GUIDE.md) | QOMM・DeFMI・zkPIの実行経路、秘密境界、状態遷移、障害時動作を端から端まで解説 |
| [`MPC_ZKPI_DEFMI_FLOW.md`](MPC_ZKPI_DEFMI_FLOW.md) | Maker/Taker事前承認、保証枠、同時RFQ、共同zkPI、口座を名指ししないDeFMI/AvalancheのDvPを、実装済みと外部未了に分けて解説 |
| [`SOURCE_FILE_INDEX.md`](SOURCE_FILE_INDEX.md) | 2026-08-29時点の第一者ソースをファイル単位で列挙し、責務・試験・変更時の影響先を対応づける |
| [`DEFMI_ZKPI_USE_CASES.md`](DEFMI_ZKPI_USE_CASES.md) | QOMMを前提にせず、DeFMIとzkPIをファンド、担保、国際決済、証券、登記などへ使う方法、既存実証との差、実装順を整理 |

## Slide decks

Three decks over the same design system. They are in Japanese; the rest of this
repository is in English, and this directory is where that is declared rather
than an exception somebody has to remember.

| file | pages | for |
|---|---:|---|
| `qomm_intro.pdf` | 19 | no prerequisites, no formulas |
| `qomm_slides.pdf` | 73 | readers who already know the field |
| `qomm_tech.pdf` | 45 | construction, proofs and measurements |

Built from `papers/qomm/slides/*.tex` in the private working tree, which is not
published. `make` there writes the PDFs here as part of building them, so the
published copies cannot fall behind the source that produced them.

Every measured number in the decks comes from `artifacts/`, and
`papers/qomm/paper_check` fails if a deck and its artifact disagree.
