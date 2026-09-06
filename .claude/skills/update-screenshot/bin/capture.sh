#!/bin/zsh
# Drive the installed toolog window and take the readme's four screenshots.
#
#   capture.sh <outdir> [scroll-rows]
#
# Leaves <outdir>/raw-*.png. Redaction is a separate step; see SKILL.md.
set -e

HERE="${0:A:h}"
BUILD="$HERE/.build"
OUT="${1:?usage: capture.sh <outdir> [scroll-rows]}"
ROWS="${2:-20}"

X=200; Y=120                       # anywhere with room for 1120x760 beside it
ROW_H=28                           # a timeline row, in points

mkdir -p "$OUT" "$BUILD"
[ -x "$BUILD/mouse" ] || swiftc -O -o "$BUILD/mouse" "$HERE/mouse.swift"

# --------------------------------------------------------------- app control

# Every control is an AX element inside the WebView, addressed by path from the
# page body. Name-based lookup over `entire contents` is not an option: the
# class comparison it needs fails silently inside its own `try`, so it matches
# nothing and reports success.
BODY='set body to UI element 1 of scroll area 1 of group 1 of group 1 of window 1'

ax() { osascript >/dev/null <<EOF
tell application "System Events" to tell process "toolog"
  $BODY
$1
end tell
EOF
}

# Click something that may not be there — a toggle already in the wanted state,
# a histogram already open. Returns non-zero instead of stopping the run.
maybe() { ax "  try
    click $1
  end try
  return name of $1" 2>/dev/null; sleep "${2:-1.5}"; }

click() { ax "  click $1"; sleep "${2:-1.5}"; }

has() { osascript >/dev/null 2>&1 <<EOF
tell application "System Events" to tell process "toolog"
  $BODY
  return name of $1
end tell
EOF
}

front() { ax 'set frontmost to true'; sleep 0.5; }
place() { ax "set position of window 1 to {$X, $Y}"; sleep 0.4; }
nav()   { click "button \"$1\" of group 1 of body" 2; }

# The pointer parks on the app bar, which has no hover state of its own. Left
# over a row it bakes a lit band into the frame, and moving the window does not
# clear it — WebKit paints `:hover` from the last event, not from where the
# cursor now is.
park() { "$BUILD/mouse" move $((X + 600)) $((Y + 48)); sleep 0.8; }

shoot() {
  front; park
  screencapture -x -R$X,$Y,1120,760 "$OUT/$1"
  echo "  $1"
}

filter() {
  front
  ax '  set value of attribute "AXFocused" of (combo box "Filter and search" of group 1 of group 2 of body) to true'
  sleep 0.3
  ax '  keystroke "a" using command down'
  ax '  key code 51'
  sleep 0.6
  # A trailing space closes the completion list, which otherwise covers the
  # first four rows of the very list the screenshot is meant to show.
  if [ -n "$1" ]; then
    ax "  keystroke \"$1 \""
    sleep 2
  fi
  # Capture is paused, but a call ingested just before it stopped can still
  # raise the "N new calls" pill. Clicking it folds the rows in and clears it.
  ax '  try
    click (first button of group 2 of body whose name contains "new call")
  end try'
  sleep 1
}

# Scroll to the top first, then down an exact number of rows: any other offset
# leaves a row clipped in half under the header.
scroll_rows() {
  "$BUILD/mouse" scrollpx $((X + 600)) $((Y + 500)) 200 25; sleep 0.8
  if [ "$1" -gt 0 ]; then
    "$BUILD/mouse" scrollpx $((X + 600)) $((Y + 500)) -$((ROW_H * 2)) $(($1 / 2))
    sleep 0.8
  fi
}

# ------------------------------------------------------------------ sequence

echo "toolog $(toolog --version | awk '{print $2}') from $(which toolog)"

pgrep -x toolog >/dev/null || { open -a /Applications/toolog.app; sleep 6; }
front; place

echo "settling the window"
nav Status
# Capture is paused for the duration, or a call ingested mid-shoot raises the
# "N new calls" pill over the rows. It is resumed at the end only if this run
# is what stopped it — the button is absent when it was already paused.
WE_PAUSED=no
if has 'button "Pause capture" of group 2 of body'; then
  click 'button "Pause capture" of group 2 of body'
  WE_PAUSED=yes
fi
click 'radio button "Light" of radio group "Theme" of group 2 of body'
nav Timeline
# Collapsed, the toggle sits one group deeper than it does open, so this is a
# path that exists only in the state it is wanted in.
maybe 'button "SHOW ACTIVITY" of group 2 of group 2 of body' || true

echo "capturing"
filter "";                 scroll_rows "$ROWS"; shoot raw-timeline.png
filter "@risk:high";       scroll_rows 0;       shoot raw-timeline-risk.png
filter "@model-risk:>=4";  scroll_rows 0;       shoot raw-timeline-model-risk.png
nav Risk;                  scroll_rows 0;       shoot raw-risk.png

echo "restoring"
nav Status
click 'radio button "System" of radio group "Theme" of group 2 of body'
if [ "$WE_PAUSED" = yes ]; then
  click 'button "Resume capture" of group 2 of body'
fi
nav Timeline

sips -g pixelWidth -g pixelHeight "$OUT"/raw-*.png | grep -E "pixel|png" | paste - - -
