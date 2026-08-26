# Japanese slide decks

Three decks over the same design system. They are in Japanese; the rest of this
repository is in English, and this directory is where that is declared rather
than an exception somebody has to remember.

| file | pages | for |
|---|---:|---|
| `qomm_intro.pdf` | 18 | no prerequisites, no formulas |
| `qomm_slides.pdf` | 59 | readers who already know the field |
| `qomm_tech.pdf` | 46 | construction, proofs and measurements |

Built from `papers/qomm/slides/*.tex` in the private working tree, which is not
published. `make` there writes the PDFs here as part of building them, so the
published copies cannot fall behind the source that produced them.

Every measured number in the decks comes from `artifacts/`, and
`papers/qomm/check_numbers.py` fails if a deck and its artifact disagree.
