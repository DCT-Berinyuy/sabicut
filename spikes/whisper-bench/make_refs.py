#!/usr/bin/env python3
"""Builds refs/<lang>.txt from the Project Gutenberg texts that fetch.sh downloads.

Each reference is the text spoken in the 600 s clip: the LibriVox intro as heard, a line
`<<<STORY`, then the chapter text from `start` through `end` (where the clip is cut). Lines
`<<<WINDOW` and `WINDOW>>>` mark the 2-minute window used for the hand-checked WER. The intro
only helps alignment and is not scored. `fixes` records hand-checked differences between the
printed text and what the reader actually said.
"""

import re
from pathlib import Path

HERE = Path(__file__).resolve().parent

SOURCES = {
    "en": {
        "gutenberg": "audio/en-alice.gutenberg.txt",
        "intro": (
            "This is a LibriVox recording. All LibriVox recordings are in the public domain. "
            "For more information or to volunteer, please visit librivox.org. "
            "Alice's Adventures in Wonderland, by Lewis Carroll. Chapter one. Down the Rabbit-Hole."
        ),
        "start": "Alice was beginning to get very tired",
        "end": "I can creep under the door;",
        "window_start": "Presently she began again.",
        "window_end": "lamps hanging from the roof.",
        "fixes": [],
    },
    "fr": {
        "gutenberg": "audio/fr-lettres.gutenberg.txt",
        "intro": (
            "Le secret de maître Cornille, des Lettres de mon moulin. Cet enregistrement "
            "LibriVox fait partie du domaine public. Enregistré par Naf. Lettres de mon moulin, "
            "par Alphonse Daudet. Le secret de maître Cornille."
        ),
        "start": "Francet Mamaï, un vieux joueur de fifre",
        "end": "éventrant les sacs, surveillant la meule,",
        "window_start": "Dans le pays on pensait que le vieux meunier",
        "window_end": "que de sacs de farine.",
        "fixes": [],
    },
}


def clean(text: str) -> str:
    text = text.replace("_", "")
    text = re.sub(r"(\s*\*){3,}", " ", text)
    return re.sub(r"\s+", " ", text).strip()


def cut(text: str, start: str, end: str) -> str:
    i = text.index(start)
    j = text.index(end, i) + len(end)
    return text[i:j]


def build(lang: str) -> str:
    src = SOURCES[lang]
    book = clean((HERE / src["gutenberg"]).read_text(encoding="utf-8"))
    body = cut(book, clean(src["start"]), clean(src["end"]))
    for old, new in src["fixes"]:
        if old not in body:
            raise SystemExit(f"{lang}: fix target not found: {old!r}")
        body = body.replace(old, new, 1)
    window = cut(body, clean(src["window_start"]), clean(src["window_end"]))
    before, after = body.split(window, 1)
    parts = [
        src["intro"], "<<<STORY", before.strip(), "<<<WINDOW", window, "WINDOW>>>", after.strip()
    ]
    return "\n".join(p for p in parts if p) + "\n"


def main() -> None:
    out_dir = HERE / "refs"
    out_dir.mkdir(exist_ok=True)
    for lang in SOURCES:
        (out_dir / f"{lang}.txt").write_text(build(lang), encoding="utf-8")
        print(f"wrote refs/{lang}.txt")


if __name__ == "__main__":
    main()
