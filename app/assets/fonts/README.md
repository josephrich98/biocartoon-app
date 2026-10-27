# Text box fonts

The typefaces offered in a text box's Font menu besides egui's own Ubuntu and Hack
(`TextFont::EMBEDDED` in `src/doc.rs`), compiled into the desktop app and loaded into the SVG /
PDF / PNG exporter. The web build fetches them from `fonts/` at startup instead (index.html copies
this folder into `dist/`), which keeps the .wasm under Cloudflare's 25 MiB limit. Regular weight only: bold and italic are synthesized, as for Ubuntu.

All are from [Google Fonts](https://fonts.google.com), unmodified (the static Regular instance the
Google Fonts CSS API serves): Tinos, Roboto Slab and Permanent Marker under the Apache License 2.0,
the rest under the SIL Open Font License 1.1. Each font's `LICENSE-*.txt` holds its license text.

To add one, put `<Name>-Regular.ttf` and its license here, add a variant at the end of `TextFont`,
a row to `TextFont::EMBEDDED`, and the variant to `TextFont::ALL` in alphabetical order (a test
checks the order).

| file | stands in for |
|---|---|
| `Arimo-Regular.ttf` | Arial / Helvetica (metric-compatible) |
| `Tinos-Regular.ttf` | Times New Roman (metric-compatible) |
| `Cousine-Regular.ttf` | Courier New (metric-compatible) |
| `Roboto-Regular.ttf`, `OpenSans-Regular.ttf`, `Lato-Regular.ttf`, `Montserrat-Regular.ttf` | common sans-serifs |
| `SourceSerif4-Regular.ttf`, `EBGaramond-Regular.ttf` | serifs |
| `ComicNeue-Regular.ttf` | Comic Sans |
| `Carlito-Regular.ttf` | Calibri (metric-compatible) |
| `Caladea-Regular.ttf` | Cambria (metric-compatible) |
| `Gelasio-Regular.ttf` | Georgia (metric-compatible) |
| `Inter`, `SourceSans3`, `Poppins`, `Nunito`, `Raleway`, `WorkSans`, `IBMPlexSans`, `DMSans`, `Barlow`, `AtkinsonHyperlegible`, `Lexend`, `Quicksand`, `Cabin`, `JosefinSans` | more sans-serifs |
| `Oswald`, `RobotoCondensed`, `BebasNeue` | condensed / display sans-serifs |
| `Lora`, `PlayfairDisplay`, `LibreBaskerville`, `CrimsonText`, `IBMPlexSerif`, `RobotoSlab`, `Arvo` | more serifs |
| `JetBrainsMono`, `SourceCodePro`, `IBMPlexMono`, `RobotoMono`, `CourierPrime` | monospace |
| `PermanentMarker`, `IndieFlower`, `PatrickHand`, `ShadowsIntoLight`, `ArchitectsDaughter` | handwriting |
