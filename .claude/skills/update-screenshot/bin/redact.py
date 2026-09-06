#!/usr/bin/env python3
"""Blur every home directory in a screenshot, and prove none survived.

Vision is asked for the token ``Users`` and whatever segment follows it, rather
than for one spelling of a name: a partly covered or misread username is still
caught, and so is ``-Users-<name>-``, the form Claude Code encodes a project
path into.

Vision's box for a *substring* is not usable here — at 13px monospace it
sometimes returns the whole line, which puts the blur over the wrong half of
the command and leaves the name beside it. So the span is measured instead.
Each row that names a home directory is cropped out on its own and re-read
upscaled, which gives a box around the command text alone; the command is
monospace, so its pitch is that box's width over its length, and the span is
arithmetic from there. Both ends are padded by a character, because a pitch
estimated across sixty characters drifts by about one.

    redact.py <in.png> <out.png>
"""

import json
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
OCR = os.path.join(HERE, ".build", "ocr")

# The INPUT column, in window points. Cropping to it keeps the second read on
# nothing but the command: no outcome glyph, no project name, no timestamp.
COLUMN = (296, 862)
SCALE = 4                 # 13px monospace is at the edge of what Vision resolves
PAD_L, PAD_R = 9, 13
BLUR = 7


def ocr(png, pattern=None):
    cmd = [OCR, png] + ([pattern] if pattern else [])
    return json.loads(subprocess.run(cmd, capture_output=True, check=True).stdout)


def rows_naming_home(png):
    """The vertical band of every line whose text names a home directory."""
    for line in ocr(png):
        if line["matches"]:
            yield line["ly"], line["lh"]


def spans_in_row(im, top, height, tmp):
    """Blur boxes for one row, measured from a crop holding only the command."""
    y0, y1 = int(top) - 4, int(top + height) + 4
    crop = im.crop((COLUMN[0], y0, COLUMN[1], y1))
    crop = crop.resize((crop.width * SCALE, crop.height * SCALE), Image.LANCZOS)
    crop.save(tmp)
    for line in ocr(tmp):
        if not line["matches"]:
            continue
        pitch = line["lw"] / max(len(line["line"]), 1)
        for m in line["matches"]:
            x0 = line["lx"] + m["loc"] * pitch
            x1 = x0 + m["len"] * pitch
            yield (m["text"],
                   COLUMN[0] + x0 / SCALE - PAD_L, y0,
                   COLUMN[0] + x1 / SCALE + PAD_R, y1)


def main(src, dst):
    im = Image.open(src).convert("RGB")
    found = []
    with tempfile.TemporaryDirectory() as d:
        tmp = os.path.join(d, "row.png")
        for top, height in rows_naming_home(src):
            for text, x0, y0, x1, y1 in spans_in_row(im, top, height, tmp):
                box = tuple(int(round(v)) for v in (x0, y0, x1, y1))
                im.paste(im.crop(box).filter(ImageFilter.GaussianBlur(BLUR)), box)
                found.append((text, box))
    im.save(dst)
    for text, box in found:
        print(f"  blurred {text!r} at {box}")
    print(f"{os.path.basename(dst)}: {len(found)} span(s) blurred")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
