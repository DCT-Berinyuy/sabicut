# Captions, hook and subtitles

`sabicut render` draws word-by-word captions and the hook over the video and writes
subtitles (SRT) next to the MP4 (PRD F6). `sabicut preview` draws the same captions and
hook on its stills, so the layout can be checked before a render. `sabicut srt` writes
only the subtitles.

```sh
sabicut preview plan.toml --at 0.5,3,8 --safe-zones   # stills with captions and hook
sabicut render plan.toml                              # plan.mp4, plan.jpg and plan.srt
sabicut srt plan.toml                                 # plan.srt only; -o - prints it
```

The drawing is done by the `sabicut-overlay` crate: words are shaped with rustybuzz (a
port of HarfBuzz: kerning, accents, ligatures) and their outlines filled and stroked with
tiny-skia. Nothing depends on the fonts installed on the machine, so the same plan,
words and frame give the same pixels every time.

## What the plan says

| Key | Meaning |
|---|---|
| `[hook]` `text`, `dur` | The hook line, shown from the start of the output for `dur` seconds |
| `[[source]]` `words` | The clip's word timings (`words.json`, from `sabicut analyze`) |
| `[captions]` `words` | Word timings of the only source, for a plan with a single source |
| `[captions]` `fixes` | Transcript fixes (`fixes.toml`), applied every time the words are read; `words.json` is never changed |
| `[captions]` `keywords` | Words always drawn in the active colour. Matching ignores capitals and the punctuation around words; a keyword of several words (`"Visual Studio"`) matches them in a row |
| `[captions]` `font` | A font file to use instead of the bundled Anton (the default) |
| `[captions]` `active` | Colour of the word being spoken and of keywords, `#RRGGBB`; gold, `#FFC400`, by default |
| `[captions]` `stroke` | Width of the black outline around the letters, in output pixels; 10 by default, 0 for none |

Captions need word timings for every source a cut uses. A source's own `words` comes
first; `captions.words` is used only when the plan has a single source, so a plan with
several sources names the word timings on each one (`sabicut validate` reports any that
is missing). Paths are relative to the plan file. `sabicut plan` writes all of this for
you, and leaves the font, colour and outline out so the defaults apply.

The subtitles need word timings too, but not `[captions]`: a plan whose sources all name
their `words` gets an SRT even without captions on the video.

## Captions

Words are placed on the output timeline with the plan's cuts. Each word is snapped to
the frame grid the way cut points are, then laid over every cut of its source:

- a word inside a removed section is **dropped**;
- a word that straddles a cut's start or end is **clamped** to the cut, so it shows only
  while its kept part plays;
- a word split by a removed section, whose two parts end up next to each other, shows
  **once**, over both parts;
- a range used twice shows its words twice.

### Pages

Captions show a few words at a time, in **pages** of one or two lines. A page ends:

- after a word that ends a sentence (`.`, `!`, `?`, `…`, also followed by quotes or
  brackets, and French `moulin !` with its no-break space);
- at a pause of 0.5 s or more;
- after a clause (`,`, `;`, `:`) once the page has 3 words;
- at 8 words;
- when the next word would need a third line.

A sentence or clause shorter than 0.5 s that the next word follows straight on does not
end a page: the end of a sentence from before a cut (`it?"`) runs on into the next page
instead of flashing by for a frame or two.

A page appears with its first word and stays until the next page starts, or until 0.4 s
after its last word when the speaker pauses. Its lines are balanced: two lines of similar
width rather than a full line and a lone word.

### The band

Captions are drawn in the **caption band**, a strip of the frame clear of the apps'
interface (see the [safe-zone guide](render.md#safe-zone-guide)):

| | On 1080×1920 | Rule |
|---|---|---|
| Columns | x 162 to 918 (756 px) | Centred, left of the action column (x ≥ 85%) |
| Bottom | y 1418 | 2% of the width above the bottom zone (y ≥ 75%) |
| Height | 240 px (y 1178 to 1418) | Two lines, with room for accents, descenders, the outline and the pop |

Sizes are fractions of the frame's reference width (the width of the largest 9:16 area
the frame holds), so captions look the same at any output size. Capital letters are 5.8%
of that width high (63 px on a 1080×1920 short) and lines are 1.5 capital heights apart.
Between two words' outlines there is a normal space. A word too wide for a line at that
size is shrunk to fit.

### Style

- Words are white with a black outline `stroke` pixels wide (outline, then fill, both
  anti-aliased, with round joins).
- The **word being spoken** is drawn in the `active` colour and **pops**: it grows to
  110% in 0.18 s with an ease-out-back curve (it overshoots to about 111% on the way),
  holds that size while it is spoken, and is drawn on top of its neighbours. It stays
  active through gaps of up to 0.25 s, so the highlight does not flicker between words.
  A popped word near the edge of the band is moved inwards, and a word as wide as the
  band grows only as far as it fits, so nothing is ever cut off.
- **Keywords** are always in the `active` colour.
- Text is shown as written in the transcript (no change of case).

## Hook

The hook is large text, centred in the same column as the captions, starting just below
the top bar (2% of the width below the top 12.5%). It is shown on the hook's frames only
(`[hook] dur` seconds from the start).

- **Size.** As large as fits: capitals from 8.5% of the reference width (92 px on a
  1080×1920 short) down to 4.5%, in at most four balanced lines, between the top bar and
  the caption band (and never below 45% of the height). The outline grows with the text.
  A hook that does not fit even at the smallest size is an error that names `hook.text`:
  keep hooks to about ten words.
- **Entrance.** Over its first 0.3 s it grows from 70% to full size with an
  ease-out-back pop and fades in, then holds still until it goes.
- **No collisions.** The hook's box always ends above the caption band, so the two never
  overlap, whatever the frame.
- Keywords are drawn in the `active` colour here too.
- When the cover (`--cover-at`) is taken after the hook has gone, the hook is drawn on
  the cover anyway.

## Subtitles (SRT)

`sabicut render` writes `<video>.srt` next to the MP4 when every source has word
timings, and `sabicut srt` writes the same file without rendering. Cues are phrases on
the output timeline, with fixes applied and no styling:

- a cue ends after a sentence, at a pause of 0.6 s or more, after a clause once it has a
  full line of text, before it would last more than 7 s, and when the next word would
  not fit in two lines of 42 characters; a phrase shorter than 0.8 s that the next one
  follows straight on runs on into the next cue instead of flashing by;
- it runs from the first frame of its first word to the last frame of its last word,
  stays on screen at least 0.8 s when the next cue leaves room, and never overlaps the
  next cue;
- lines are balanced; times are `HH:MM:SS,mmm`, rounded to the millisecond from the
  frame boundaries.

## Fonts

Captions use **Anton Regular** (by Vernon Adams and the Anton Project Authors), a bold
condensed display face with French accents, embedded in the `sabicut` binary. It is
licensed under the [SIL Open Font License 1.1](../crates/overlay/fonts/OFL.txt), which
allows bundling it with software; the font file and its license are in
[`crates/overlay/fonts/`](../crates/overlay/fonts/).

`captions.font` names another OpenType or TrueType font, relative to the plan file. It
must have outlines for Latin text (not a bitmap or emoji font). Characters the font
cannot draw (an emoji, say) show as its empty box, and `render` and `preview` list them.
Emoji and stickers are not supported (PRD §4).

## How it is drawn

PRD §8 asks for only the caption band to be drawn, never whole frames:

- For every output frame, the render draws the band (756×240 RGBA on a 1080×1920 short,
  0.73 MB, against 1.7 MB for the PRD's 1080×400 estimate) and pipes it to the encoding ffmpeg, from a thread of its own: ffmpeg
  opens its inputs one after another and reads ahead in each, so one thread writing
  both pipes in turn could wait forever. ffmpeg's input queue keeps the band at most a
  few frames ahead of the video. The band goes in on the encoder's standard output,
  connected to the reading end of a pipe (`-i pipe:1`): standard input already carries
  the video, and this needs no named pipe or extra file descriptor.
- Each word is drawn once per colour and size and kept, so most frames are a few copies;
  a frame that looks like the one before is not drawn again.
- The hook's animated pictures (0.3 s worth) go in a scratch file; the last one is held
  until the hook ends.
- Both are converted to the video's colours explicitly (BT.709, limited range, 4:2:0
  with alpha) and placed with `overlay=x:y`, the hook with `enable` on its frames only.
- Preview stills take the same pictures for their frame, as one-picture raw RGBA files,
  through the same `overlay` chain. The safe-zone guide is drawn on top.

### Text stack

The PRD named cosmic-text. Sabicut uses rustybuzz and its outline parser (ttf-parser)
directly instead, with tiny-skia, because:

- captions are laid out word by word (each word is timed, coloured and scaled on its own),
  so cosmic-text's line breaking, rich-text spans and editing are not needed;
- cosmic-text finds fonts through fontdb, which can fall back to fonts installed on the
  machine, and the same plan could then come out differently on two laptops;
- it is much smaller (see the report in the pull request for the measured download and
  binary sizes).

rustybuzz does what matters for captions: kerning (GPOS and `kern`), mark positioning
for accents, composing `e` + combining acute into `é`, and spaces such as the French
no-break space.
