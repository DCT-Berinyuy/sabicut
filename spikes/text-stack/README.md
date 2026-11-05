# Text stack spike (F6)

PRD §8 named cosmic-text for drawing captions. `sabicut-overlay` uses rustybuzz (shaping:
kerning, accents, ligatures) and its outline parser instead, with tiny-skia drawing the
outlines. This spike measures what each stack costs: three small binaries draw the same
outlined, kerned French caption line (`Été comme hiver, le moulin tourne !`, Anton at
73 px per em, 10 px outline) with tiny-skia, and print a checksum of the ink.

| Binary | Stack | Stripped size | Over the baseline | Release build (2 jobs, cold deps) |
|---|---|---|---|---|
| `base` | tiny-skia and the embedded font, no text (an outlined rectangle) | 788,608 B | | 7 s |
| `rb` | rustybuzz 0.20.1 + tiny-skia 0.11.4 | 1,571,648 B | +783,040 B | 25 s |
| `ct` | cosmic-text 0.19.0 (harfrust, swash; no fontconfig) + tiny-skia 0.11.4 | 3,070,448 B | +2,281,840 B | 2 min 16 s |

Both text binaries put the same amount of ink on the picture (35.74 M against 35.54 M,
0.6% apart), so both really draw the line.

Download size, crates only, on an empty cache, for Linux (from `cargo tree` and the
`.crate` files):

| Stack | Crates | Download |
|---|---|---|
| tiny-skia alone | 8 | 414,142 B |
| rustybuzz + tiny-skia | 18 | 1,251,816 B |
| cosmic-text + tiny-skia | 46 | 8,107,423 B |

The cosmic-text crate alone is 2.96 MB (it ships test fonts and screenshots), and its
dependencies include two versions each of read-fonts, skrifa and font-types.

Measured on 5 November 2026, Rust 1.98.1, x86_64 Linux.

```sh
cargo build --release --bin base
cargo build --release --features rb --bin rb
cargo build --release --features ct --bin ct
ls -l target/release/{base,rb,ct}
```
