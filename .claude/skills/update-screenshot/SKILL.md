---
name: update-screenshot
description: Retake the four screenshots in README.md against the installed toolog app — the timeline, the risk review, and the timeline under @risk:high and @model-risk:>=4 — then blur the home directory out of them and prove it is gone. Use after a release, or after any change to the window that the readme's images now misrepresent.
---

# Updating the readme's screenshots

```
.claude/skills/update-screenshot/bin/update.sh [scroll-rows]
```

That drives the installed app, takes the four frames, blurs every home
directory, reads each image back to prove none survived, and copies them over
`docs/screenshots/`. Nothing is installed unless every image passes the
read-back. Then **look at all four** — the checks catch a leak, not an ugly
frame.

The four filenames are fixed; `README.md` links them by name:

| File | What it shows |
|---|---|
| `timeline.png` | The timeline, unfiltered, scrolled far enough down that the RISK and MODEL columns are not empty |
| `risk.png` | The risk review, at the top |
| `timeline-risk.png` | `@risk:high` |
| `timeline-model-risk.png` | `@model-risk:>=4` |

## Before running it

- **`toolog --version` must be the version being documented.** The frames are
  taken of `/Applications/toolog.app`, and the app bar prints its version into
  every one of them. The last set went out reading `v1.1.0` because it was shot
  from a development build during Phase 13.
- **The store needs to be worth photographing.** `@risk:high` over a fresh
  store is an empty list.
- The terminal running this needs Accessibility and Screen Recording
  permission. Without the first, nothing is clicked; without the second, every
  frame is of the desktop.
- The frames are `1120x760` — the window's own `inner_size` — which assumes a
  1:1 main display. On a Retina display `screencapture -R` returns twice that,
  and the images will not match the set already in the repository.

## What it does, in order

Capture (`capture.sh`) launches the app if it is not running, moves the window
to a known corner, then: pauses capture, switches the theme to light, opens the
activity histogram, shoots the four frames, and **puts the theme and capture
back the way it found them**. It resumes capture only if it was the thing that
paused it.

Redaction (`redact.py`) asks Vision for the token `Users` and whatever segment
follows it — not for one spelling of a name, so a partly covered or misread
username is still caught, and so is `-Users-<name>-`, the form Claude Code
encodes a project path into.

The read-back (`verify.py`) reads every redacted image again, zoomed, and fails
on the token, on any fragment of the name, or on a tail of the name left
touching `-Projects`.

## The four things that cost an afternoon

**Vision's bounding box for a substring is not usable at this text size.**
`boundingBox(for:)` is fine at its left edge and wrong often enough at its
right that the first pass put the blur over `SP=/tmp/claude-501` and left the
name legible beside it. Each row is now cropped to the INPUT column, re-read
upscaled, and the span computed from the monospace pitch — the width of that
line's box over its length. A pitch estimated across sixty characters drifts by
about one, so both ends are padded by a character.

**System Events cannot move the pointer.** `click at {x, y}` fails with -25208.
Everything in the window is clicked as an AX element instead, and the pointer
is driven by `mouse.swift` through CoreGraphics.

**Warping the pointer does not clear `:hover`.** WebKit paints it from the last
event, not from where the cursor is, so the cursor can sit on the Dock while a
row inside the window stays lit — and moving the window does not clear it
either. Every warp is followed by a real `mouseMoved`. The pointer parks on the
app bar, which has no hover state of its own.

**Name-based AX lookup silently matches nothing.** Walking `entire contents`
and testing `class of e is in {button, radio button}` throws inside its own
`try`, so the loop finds nothing and reports success. Controls are addressed by
path from the page body instead. Two paths shift with the page and are the ones
to re-check if a click stops landing:

| Control | Path from `body` |
|---|---|
| Tabs | `button "Timeline"/"Risk"/"Status" of group 1` |
| Status page controls, theme | `group 2` |
| Query box | `combo box "Filter and search" of group 1 of group 2` |
| Activity toggle, collapsed | `button "SHOW ACTIVITY" of group 2 of group 2` |
| Activity toggle, open | `button "HIDE ACTIVITY" of group 2` |

## Judgement calls the script cannot make

**The scroll offset**, the one argument. The unfiltered timeline defaults to 20
rows down, because at the top of the list are the calls this session just made
— which on a documentation day are all low-scoring greps, and a caption about
two verdict columns illustrated by two empty columns is worse than no
screenshot. Pass a different number, or `0` for the top of the list, and look
at what you get. Rows are 28px, and the scroll lands on an exact multiple so no
row is clipped in half under the header.

**A trailing space** is typed after every filter, which closes the completion
list. Without it the completion covers the first four rows of the list the
screenshot exists to show.

**Light theme.** The set in the repository is light. Nothing but consistency
argues for it; if the set is ever redone in dark, change it in `capture.sh` and
change all four at once.
