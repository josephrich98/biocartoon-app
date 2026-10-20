# Notices

Copyright (c) 2026 Joseph Rich. Licensed under the GNU AGPL v3.0 or later; see [LICENSE](LICENSE).

## BioCartoon's own work

The BioCartoon editor, web app and website source code, logo and icons, and
documentation are free software: you can redistribute and/or modify them under the terms of the
GNU Affero General Public License as published by the Free Software Foundation, either version 3
of the License, or (at your option) any later version. They are distributed WITHOUT ANY WARRANTY;
see [LICENSE](LICENSE). If you run a modified version as a network service, you must offer its
users the corresponding source code (AGPL section 13).

The asset catalog's search engine (the `biocartoon-catalog` crate the editor is built with), the
data pipeline and the submissions Worker are in [biocartoon](https://github.com/josephrich98/biocartoon),
under the BSD 2-Clause License.

## Spelling dictionaries

The spelling checker bundles the SCOWL English (US) Hunspell dictionary (MIT/BSD-style license)
and the hunspell-en-med-glut medical dictionary (GNU GPL v3, compatible with the AGPL), see
[app/assets/dict](app/assets/dict/README.md).

## Third-party illustrations and icons

The illustration and icon libraries BioCartoon searches and displays are **not**
covered by the BioCartoon license. Each asset stays under its own upstream license, and the
asset database (`license`, `license_url`, `credit` columns) records the license and credit for
every item. When you use an asset in a figure, follow that asset's license, e.g. credit the
creator for CC BY items and share adaptations of CC BY-SA items under CC BY-SA.

| source | license |
|---|---|
| [NIAID NIH BioArt Source](https://bioart.niaid.nih.gov) | Public Domain / CC BY (per item) |
| [Bioicons](https://bioicons.com) | CC0 / CC BY / CC BY-SA / MIT / BSD (per icon) |
| [Health Icons](https://healthicons.org) | CC0 |
| [OpenMoji](https://openmoji.org) | CC BY-SA 4.0 |
| [Reactome icon library](https://reactome.org/icon-lib) | CC BY 4.0 |
| [SciDraw](https://scidraw.io) | CC BY / CC0 (per item) |
| [Servier Medical Art](https://smart.servier.com) | CC BY 4.0 |

## User-contributed artwork

People who contribute templates, illustrations or icons keep the copyright in their work.
BioCartoon does not own it. Each contribution is published under the license its creator
chose when submitting it (CC0 or CC BY 4.0), and that license, not the AGPL, governs its use.
The BioCartoon license covers only the project's own code and the artwork its author made.

Figures you make with BioCartoon are yours. The BioCartoon license does not restrict what
you do with your own output, including publishing it in commercial journals, but the
third-party assets inside a figure keep their own licenses.

Third-party software dependencies are under their own licenses; see `app/Cargo.lock` and
`package.json`.
