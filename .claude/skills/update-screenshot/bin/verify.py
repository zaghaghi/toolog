#!/usr/bin/env python3
"""Read a redacted screenshot back and fail if anything legible survived.

The whole image is read once, then every row band is read again upscaled — the
same second pass the redactor uses, because that is the pass that can actually
resolve this text size. A first pass that says "clean" over a name a zoomed
pass can read is worse than no check at all.

Three things fail it: the token and the segment after it, any fragment of the
name, and a tail of the name left touching the next path segment. That last one
is the failure this check exists for — a pitch estimate one character short
leaves a single letter against ``-Projects``, which no search for the name
would ever match.

Exits non-zero on a leak.

    verify.py <image.png> [name-fragment ...]
"""

import json
import os
import re
import subprocess
import sys
import tempfile

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
OCR = os.path.join(HERE, ".build", "ocr")
SCALE = 4

PATTERNS = [
    r"Users[/\-][A-Za-z0-9._]",       # the token and whatever segment follows it
    r"[A-Za-z0-9][/\-]Projects",      # a tail of the name touching the next segment
]


def ocr(png):
    return json.loads(subprocess.run([OCR, png], capture_output=True, check=True).stdout)


def main(png, fragments):
    # A bare GitHub handle is not a leak — the readme prints it in the install
    # line — so only fragments given on the command line are searched for.
    suspect = re.compile("|".join(PATTERNS + [re.escape(f) for f in fragments]), re.I)

    im = Image.open(png).convert("RGB")
    lines = ocr(png)
    bad = [("full", l["line"]) for l in lines if suspect.search(l["line"])]

    with tempfile.TemporaryDirectory() as d:
        tmp = os.path.join(d, "row.png")
        for line in lines:
            y0 = max(int(line["ly"]) - 4, 0)
            y1 = min(int(line["ly"] + line["lh"]) + 4, im.height)
            crop = im.crop((0, y0, im.width, y1))
            crop = crop.resize((crop.width * SCALE, crop.height * SCALE), Image.LANCZOS)
            crop.save(tmp)
            bad += [("zoom", l["line"]) for l in ocr(tmp) if suspect.search(l["line"])]

    for where, text in bad:
        print(f"  LEAK ({where}): {text!r}")
    print(f"{os.path.basename(png)}: {'FAIL' if bad else 'clean'}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2:]))
