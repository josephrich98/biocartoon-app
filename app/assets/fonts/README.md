# Text box fonts

The typefaces offered in a text box's Font menu besides egui's own Ubuntu and Hack
(`TextFont::EMBEDDED` in `src/doc.rs`), compiled into the app and loaded into the SVG / PDF / PNG
exporter. Regular weight only: bold and italic are synthesised, as for Ubuntu.

All are from [Google Fonts](https://fonts.google.com): Tinos under the Apache License 2.0
(Copyright 2010 Google Corporation), the rest under the SIL Open Font License 1.1. Each font's
`LICENSE-*.txt` holds its license text.

| file | stands in for |
|---|---|
| `Arimo-Regular.ttf` | Arial / Helvetica (metric-compatible) |
| `Tinos-Regular.ttf` | Times New Roman (metric-compatible) |
| `Cousine-Regular.ttf` | Courier New (metric-compatible) |
| `Roboto-Regular.ttf`, `OpenSans-Regular.ttf`, `Lato-Regular.ttf`, `Montserrat-Regular.ttf` | common sans-serifs |
| `SourceSerif4-Regular.ttf`, `EBGaramond-Regular.ttf` | serifs |
| `ComicNeue-Regular.ttf` | Comic Sans |
