#!/bin/zsh
# Retake the readme's four screenshots, blur the home directory, prove it is
# gone, and install them.
#
#   update.sh [scroll-rows]
#
# Nothing is installed unless every image passes the read-back check.
set -e

HERE="${0:A:h}"
BUILD="$HERE/.build"
REPO="${HERE:h:h:h:h}"
DEST="$REPO/docs/screenshots"
WORK="${TMPDIR:-/tmp}/toolog-screenshots.$$"

mkdir -p "$BUILD" "$WORK"
trap 'rm -rf "$WORK"' EXIT

[ -x "$BUILD/ocr" ] || swiftc -O -o "$BUILD/ocr" "$HERE/ocr.swift"

"$HERE/capture.sh" "$WORK" "${1:-20}"

echo "\nredacting"
for f in "$WORK"/raw-*.png; do
  python3 "$HERE/redact.py" "$f" "${f/raw-/red-}"
done

echo "\nreading back"
# The username is passed in rather than written down: the check is for this
# machine's home directory, whoever is running it.
NAME="$(basename "$HOME")"
for f in "$WORK"/red-*.png; do
  python3 "$HERE/verify.py" "$f" "$NAME"
done

echo "\ninstalling into ${DEST#$REPO/}"
for f in "$WORK"/red-*.png; do
  cp "$f" "$DEST/$(basename "${f#*red-}")"
done
git -C "$REPO" status --short "$DEST"
