"""Builds the math fonts under apps/web/assets/fonts/math from KaTeX's fonts.

Usage: python3 scripts/math/build_math_fonts.py <katex-fonts-dir>
(KaTeX 0.18's `dist/fonts`, or katex-native's `fonts/`: the same files.)

Every KaTeX font file declares itself upright and regular, so a font system that
matches by family, weight and style cannot tell KaTeX_Main Regular from Italic.
Each file is renamed to a family of its own. Renaming makes these Modified
Versions under the SIL Open Font License 1.1, which forbids the Reserved Font
Names `KaTeX_*` on them; the names here are `T3Math_*`. Faces without an `m` also
map `m` to their space glyph (see `rename`). Outlines, metrics and the copyright
notice are unchanged, and OFL.txt goes with the files.

It also writes `spacer.ttf`: blank glyphs at U+E000 + k that advance 2^k / 512 em,
and a blank U+E010 that does not advance, so 16 of them spell out any width up
to 64 em for an inline formula's slot in a text line.
"""

import os
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

OUT = os.path.join(os.path.dirname(__file__), "../../apps/web/assets/fonts/math")
SPACER_UNITS = 512
SPACER_GLYPHS = 15


def rename(source: str, target_dir: str) -> None:
    font = TTFont(source)
    stem = os.path.splitext(os.path.basename(source))[0]
    family = stem.replace("KaTeX_", "T3Math_")
    names = font["name"]
    for record in list(names.names):
        if record.nameID in (1, 3, 4, 6, 16, 17, 18, 21, 22):
            names.removeNames(nameID=record.nameID)
    for name_id, value in ((1, family), (2, "Regular"), (3, f"{family}: modified from {stem}"), (4, family), (6, family)):
        names.setName(value, name_id, 3, 1, 0x409)
        names.setName(value, name_id, 1, 0, 0)
    # GPUI's Linux text system only takes a face with an `m` as a primary font. KaTeX's
    # delimiter and symbol faces have none, and KaTeX never sets an `m` in them, so `m`
    # maps to the blank space glyph there.
    for table in font["cmap"].tables:
        if table.isUnicode() and ord("m") not in table.cmap:
            table.cmap[ord("m")] = table.cmap.get(0x20) or table.cmap.get(0xA0) or ".notdef"
    font.save(os.path.join(target_dir, f"{family}.ttf"))


def spacer(target_dir: str) -> None:
    names = [".notdef", "blank"] + [f"space{k}" for k in range(SPACER_GLYPHS)]
    builder = FontBuilder(SPACER_UNITS, isTTF=True)
    builder.setupGlyphOrder(names)
    # `m`: GPUI's Linux text system only takes a face with one as a primary font.
    builder.setupCharacterMap({ord("m"): "blank", 0xE010: "blank", **{0xE000 + k: f"space{k}" for k in range(SPACER_GLYPHS)}})
    empty = TTGlyphPen(None).glyph()
    # Font loaders reject a glyf table with no outline at all; .notdef gets a box nothing maps to.
    pen = TTGlyphPen(None)
    pen.moveTo((0, 0)); pen.lineTo((0, 100)); pen.lineTo((100, 100)); pen.lineTo((100, 0)); pen.closePath()
    builder.setupGlyf({name: pen.glyph() if name == ".notdef" else empty for name in names})
    builder.setupHorizontalMetrics({".notdef": (100, 0), "blank": (0, 0), **{f"space{k}": (1 << k, 0) for k in range(SPACER_GLYPHS)}})
    builder.setupHorizontalHeader(ascent=0, descent=0)
    builder.setupNameTable({"familyName": "T3MathSpacer", "styleName": "Regular", "uniqueFontIdentifier": "T3MathSpacer", "fullName": "T3MathSpacer", "psName": "T3MathSpacer", "version": "Version 1.0"})
    builder.setupOS2(sTypoAscender=0, sTypoDescender=0, usWinAscent=0, usWinDescent=0, sxHeight=0, sCapHeight=0)
    builder.setupPost()
    builder.save(os.path.join(target_dir, "spacer.ttf"))


def main() -> None:
    source_dir = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    for file in sorted(os.listdir(source_dir)):
        if file.startswith("KaTeX_") and file.endswith(".ttf"):
            rename(os.path.join(source_dir, file), OUT)
    with open(os.path.join(source_dir, "OFL.txt")) as license_text, open(os.path.join(OUT, "OFL.txt"), "w") as out:
        out.write(license_text.read())
        out.write(
            "\n-----------------------------------------------------------\n"
            "The T3Math_* files are Modified Versions of the KaTeX_* fonts above: their name\n"
            "tables changed, so a font system can tell each face apart, and faces without an\n"
            "`m` map it to their space glyph. spacer.ttf holds\n"
            "blank glyphs only and is not derived from them.\n"
        )
    spacer(OUT)


if __name__ == "__main__":
    main()
