#!/usr/bin/env python3
"""Rebuild the embedded font (`src/assets/NotoSansJP-VF.ttf`) as a subset.

The web build embeds a font because browsers expose no font database (see
`src/embedded_font.rs`). Shipping Noto Sans JP whole costs ~9.6 MB, which
dominates the wasm bundle -- and "Latin + Japanese" does not help by itself,
because Japanese *is* the mass of the font:

    full font                                9.59 MB
    Latin + kana + every kanji               8.24 MB   <- 14% off, pointless
    Latin + kana only                        0.50 MB
    Latin + kana + kanji this repo uses      0.56 MB   <- what this builds

So the lever is *which kanji*, and the honest answer for a demo whose Japanese
text is entirely string literals in this repository is: exactly those.

    python tools/subset_font.py --source path/to/NotoSansJP-VF.ttf

Get the source font from the Noto project (it is not kept in this repo -- the
whole point is to not carry 9.6 MB):

    https://github.com/notofonts/noto-cjk/raw/main/Sans/Variable/TTF/Subset/NotoSansJP-VF.ttf

Requires `pip install fonttools`.

# What is kept, and why

* **Latin, punctuation, and full kana ranges** unconditionally. They are cheap
  (0.5 MB together) and they are what arbitrary runtime text is most likely to
  contain, so keeping them costs little and avoids a whole class of surprise.
* **Only the CJK ideographs that appear in this repository's `.rs` and `.html`
  sources.** That includes comments as well as UI strings -- a deliberate
  superset, since narrowing it further would save kilobytes and add a way to be
  wrong.
* **The variable weight axis.** Instancing to a single weight would halve the
  result again (0.56 -> 0.32 MB), but fontique applies no synthetic bold when
  the fallback chain has no true face for a requested weight, so bold text
  would silently stop being bold. 0.24 MB is not worth that.

# The failure mode to know about

A character that is not in the subset renders as tofu, silently -- there is no
error and no log line. **Re-run this after adding Japanese text to the demo.**
"""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
DEST = REPO_ROOT / "matcha-ecs-widgets" / "src" / "assets" / "NotoSansJP-VF.ttf"

SCANNED_SUFFIXES = {".rs", ".html"}
SKIPPED_DIRS = {"target", "dist", ".git"}

# Basic Latin, Latin-1, general punctuation, the euro, arrows and the common
# mathematical operators. The arrows earn their place: "->" written as an actual
# arrow turns up throughout this codebase's UI strings and comments.
LATIN = (
    "U+0020-007E,U+00A0-00FF,U+2000-206F,U+20AC,"
    "U+2190-21FF,U+2200-22FF,U+2713-2714,U+25A0-25FF"
)
# CJK symbols/punctuation, hiragana, katakana, halfwidth and fullwidth forms.
KANA = "U+3000-303F,U+3040-309F,U+30A0-30FF,U+FF00-FFEF"
# The ranges scanned for "kanji actually used": CJK unified ideographs and ext A.
IDEOGRAPH_RANGES = ((0x3400, 0x4DBF), (0x4E00, 0x9FFF))


def ideographs_used() -> set[str]:
    found: set[str] = set()
    for path in REPO_ROOT.rglob("*"):
        if path.suffix not in SCANNED_SUFFIXES:
            continue
        if SKIPPED_DIRS.intersection(path.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        found.update(
            c
            for c in text
            if any(lo <= ord(c) <= hi for lo, hi in IDEOGRAPH_RANGES)
        )
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        required=True,
        type=pathlib.Path,
        help="the full Noto Sans JP variable font to subset",
    )
    parser.add_argument("--output", type=pathlib.Path, default=DEST)
    args = parser.parse_args()

    if not args.source.is_file():
        print(f"no such file: {args.source}", file=sys.stderr)
        return 1

    used = ideographs_used()
    print(f"ideographs used in {REPO_ROOT}: {len(used)}")

    unicodes = ",".join(
        [LATIN, KANA] + [f"U+{ord(c):04X}" for c in sorted(used)]
    )

    subprocess.run(
        [
            sys.executable,
            "-m",
            "fontTools.subset",
            str(args.source),
            f"--unicodes={unicodes}",
            f"--output-file={args.output}",
            # Keep every OpenType feature: parley enables shaping features per
            # script, and dropping them is how ligatures and kerning quietly
            # stop happening.
            "--layout-features=*",
            # Nothing here hints at the sizes this renders at, and the tables
            # are dead weight in a glyph-atlas pipeline.
            "--no-hinting",
        ],
        check=True,
    )

    size = args.output.stat().st_size
    print(f"wrote {args.output} ({size / 1e6:.2f} MB)")

    # The kept ideographs are chosen from the sources, but everything else is
    # kept by *range* — so a character can still fall outside the subset, and
    # the runtime symptom is a silent tofu. Check rather than hope.
    missing = uncovered(args.output, args.source)
    if missing:
        print("\ncharacters used in sources but NOT in the subset:", file=sys.stderr)
        for code, where in sorted(missing.items()):
            print(f"  U+{code:04X}  e.g. {where}", file=sys.stderr)
        print("widen LATIN/KANA above and re-run.", file=sys.stderr)
        return 1

    print("every non-ASCII character used in this repo is covered")
    return 0


def uncovered(font_path: pathlib.Path, source_path: pathlib.Path) -> dict[int, str]:
    """Codepoints in sources that subsetting dropped.

    Compared against the *source* font, not against Unicode: a character Noto
    Sans JP never had (emoji, for one) is not something this script can fix, and
    reporting it would be noise that trains the reader to ignore the check.
    """
    from fontTools.ttLib import TTFont

    cmap = set(TTFont(font_path).getBestCmap())
    available = set(TTFont(source_path).getBestCmap())
    missing: dict[int, str] = {}
    for path in REPO_ROOT.rglob("*"):
        if path.suffix not in SCANNED_SUFFIXES:
            continue
        if SKIPPED_DIRS.intersection(path.parts):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for char in text:
            code = ord(char)
            if code > 0x7F and code in available and code not in cmap:
                missing.setdefault(code, str(path.relative_to(REPO_ROOT)))
    return missing


if __name__ == "__main__":
    raise SystemExit(main())
