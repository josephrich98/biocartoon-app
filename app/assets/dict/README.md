# Spelling dictionaries

Used by the Spelling and grammar window (`src/spell.rs`), compiled into the app.

| file | what | source | license |
|---|---|---|---|
| `en_US.aff`, `en_US.dic.gz` | General English (US), Hunspell format | [SCOWL](http://wordlist.aspell.net) via [wooorm/dictionaries](https://github.com/wooorm/dictionaries/tree/main/dictionaries/en) | SCOWL (MIT/BSD-style), see `LICENSE-en_US.txt` |
| `en_med.dic.gz` | Medical and biomedical terms, Hunspell format using en_US's affix flags (header removed) | [hunspell-en-med-glut](https://github.com/glutanimate/hunspell-en-med-glut) | GNU GPL v3, see `LICENSE-en_med.txt` |
| `biology.txt` | Molecular and cell biology, genomics and lab words the two lists miss | BioCartoon | AGPL v3 or later, like the app |

The `.dic` files are gzipped to keep the binary small. To add a biology word, add a line to
`biology.txt`.
