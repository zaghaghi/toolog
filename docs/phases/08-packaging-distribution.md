# Phase 8 — Packaging & distribution → **v1.0**

**Goal:** the brief's second constraint, delivered — dead simple installation, one artifact, by
download or `brew`.

**Depends on:** all previous phases.
**Governed by:** [ADR-0001](../adr/0001-tauri-2-for-the-desktop-shell.md),
[ADR-0008](../adr/0008-local-only-zero-egress.md).

## Tasks

- [x] **8.1** Universal macOS binary (`aarch64-apple-darwin` + `x86_64-apple-darwin`), `.app`
  bundle, `.dmg`. Reproducible from `just bundle`.

  `just bundle` builds the window, installs the pinned Tauri CLI if it is missing, adds both
  targets and produces `toolog_1.0.0_universal.dmg` (11.6 MB) around a 27 MB `.app`.
  `lipo -archs` reports `x86_64 arm64`.

  **The bundle had no `.icns`.** It would have shipped with a generic Finder icon and nothing
  in the build would have said so. Since the mark is flat geometry, it was measured off the
  committed 512 px PNG by sub-pixel edge coverage and re-rendered at all ten slots, including
  the 1024 one that never existed. Re-rendering at 512 and diffing the original gives a mean
  delta of 0.078/255, all of it in the corner arc.

  **`LSUIElement` moved into `Info.plist`.** `app.rs` sets `ActivationPolicy::Accessory`, but
  that runs after AppKit has already decided to show a Dock icon, so the launch flickered one
  up and then removed it.

- [x] **8.2** Codesign with a Developer ID certificate, notarize, and staple.

  Signed and verified locally: hardened runtime (`flags=0x10000`), the full
  `Developer ID → Developer ID CA → Apple Root` chain, a secure timestamp, and
  `codesign --verify --strict` clean. `just verify-bundle` asks the four questions a user's
  Mac asks, separately, because three of them fail quietly.

  **The entitlements file is deliberately empty, and asserted so.** Entitlements under the
  hardened runtime are *relaxations*; toolog asks for none, so library validation stays on and
  JIT and unsigned executable memory stay off. `the_shipped_app_asks_for_no_entitlements`
  keeps that true.

  **`plutil` accepts entitlement files that `codesign` rejects.** XML forbids two consecutive
  hyphens inside a comment; `plutil -lint` calls such a file valid, and `codesign` fails with
  `AMFIUnserializeXML: syntax error` and then signs the app *without* the entitlements.
  Writing a command line in a comment is the natural way to hit it. Now a test.

  **Notarization and the clean-machine check are the owner's to run**, and are the one part of
  this phase not verified here: they need the App Store Connect issuer ID, and a machine that
  has never seen this certificate. `just notarize` and the release workflow do it; until a
  notarized build has been opened on a clean Mac, the "no Gatekeeper warning" exit criterion
  is designed for, not demonstrated.

- [x] **8.3** Homebrew cask in a tap.

  `packaging/homebrew/toolog.rb`, regenerated from the built artifact by `just cask` so the
  version and checksum are never typed in. Three stanzas carry real decisions:

  - `binary` — ADR-0007 ships one artifact that is both the app and the CLI, so without it
    `toolog doctor` would not exist on the PATH of anyone who installed with brew, and half
    the documented commands would be unreachable.
  - `uninstall script:` — `brew uninstall` removes an application; it knows nothing about the
    six variables this one added to `~/.claude/settings.json`. Running toolog's own
    uninstaller first is what makes the byte-for-byte promise true through the package
    manager and not only through the terminal.
  - `zap` — the other half of that: `brew uninstall --zap` is how someone asks for the
    history to go too, mapping onto `toolog uninstall --delete-data`.

  **Not yet published**: the cask's `sha256` has to be the checksum of the *released*
  artifact, so the tap is populated after the release exists, not before.

- [x] **8.4** GitHub Releases with checksums and release notes; CI builds and signs on tag.

  `.github/workflows/release.yml`, tag-driven, which **refuses to run if the tag disagrees
  with `Cargo.toml`** — a release named after a version the binary does not report is worse
  than no release. It imports the certificate into a throwaway keychain, signs, notarizes,
  staples, then asserts against the artifact rather than the config: universal, hardened
  runtime, no entitlements, `spctl` clean.

  **Tauri notarizes the `.app` but only *signs* the `.dmg` it puts the app inside**, so a
  downloaded `.dmg` fails Gatekeeper even though the app within it would pass. The container
  is notarized in its own right. (Learned in `project-spy`, carried over rather than
  rediscovered.)

- [x] ~~**8.5** `tauri-plugin-updater` with signed manifests.~~ **Evaluated and declined.**

  ADR-0008 had reserved an update check as the one permitted exception. Phase 8 declined to
  take it, so there is **no** exception and v1.0 makes no network call at all. Three reasons,
  none visible at Phase 0: the guarantee had become structural in 7.7 and an updater's
  `reqwest` would be linked whether the switch was on or off; a Homebrew cask whose app
  self-updates has to opt out of `brew upgrade` managing it; and "no network calls, except
  one" is a materially weaker sentence than "no network calls".

  Enforced, not just documented: `tauri-plugin-updater` and `self_update` are named in
  `OUTBOUND_CLIENTS` in `crates/toolog-cli/tests/egress.rs`. Full reasoning in the addendum
  to [ADR-0008](../adr/0008-local-only-zero-egress.md).

- [x] **8.6** **Uninstall path** — as carefully built as the install.

  `toolog uninstall`, previewed then `--apply`, plus the same operation on the Status page.

  The interesting half is the settings file. A byte-identical restore is only safe if nothing
  else changed since the backup was taken; restoring over a hook the user added afterwards
  would be worse than an imperfect byte match. So the two are computed and **compared** —
  strip our keys from the file as it stands, read the oldest backup, restore byte for byte if
  they agree as JSON, and otherwise remove only our keys and say why. The report names which
  path it took either way.

  Two smaller decisions: a `settings.json` that holds nothing but our keys and has no backup
  is **deleted**, because it did not exist before the install; and history is **kept by
  default**, because removing the tool and destroying the record it collected are different
  decisions.

- [x] ~~**8.7** Linux `.AppImage` and `.deb` — stretch goal.~~ **Cut.**

  Dropped rather than attempted, so the claim in 1.1 is untested rather than disproved. The
  only macOS assumption that would need work is the LaunchAgent, which needs a systemd user
  unit equivalent on both the install and the uninstall path; `launchagent::is_supported()`
  already gates it. Recorded as a known gap in the README.

- [x] **8.8** `README` with the two-lane architecture diagram, the privacy posture up front,
  and honest limitations.

  Install, verify and uninstall are the first three things on the page, with measured first-run
  numbers. Six limitations are stated rather than left to be discovered — macOS only, cost data
  only for live-captured sessions, capture stopping when the app is quit, no update
  notification, lossy project-directory decoding, and a database any reader of the disk can
  open.

  **Screenshots of the four views are outstanding**, and deliberately so: the only store on
  this machine large enough to photograph is the owner's real work, and its project names,
  commands and paths do not belong in a public README. It needs either a synthetic corpus
  built for the purpose or the owner's own review of what each frame shows.

- [x] **8.9** First-run experience measured end to end.

  Measured against a real 50-file, 66 MB corpus, on the owner's machine:

  | Step | Time |
  |---|---|
  | `.dmg` mount, copy, unmount | 14.2 s |
  | `toolog doctor` | 0.6 s |
  | `toolog backfill` — 16,257 records, 3,729 tool calls, 41 sessions | 2.0 s |
  | First useful answer (`toolog usage`, `toolog verify`) | 0.01 s |
  | **Total machine time** | **~17 s** |

  Comfortably inside the two-minute target, and the shape of it is worth noting: **83% of the
  wall clock is `hdiutil` verifying the disk image**, not anything this project wrote. The
  import — the part that looked like the risk — is 2 seconds. The rest of the two minutes is
  human: reading what the tool is about to capture, and deciding to allow it.

## Exit criteria

- [x] `brew install --cask zaghaghi/tap/toolog` — cask written and its uninstall path tested;
  **publication waits on the release**, and a clean-machine install has not been performed.
- [x] The first-run wizard configures telemetry, backfills history and lands on a populated
  timeline without the user opening a terminal (Phase 4's wizard, unchanged and still wired).
- [x] Uninstall leaves `~/.claude/settings.json` byte-identical to its pre-install state —
  asserted by `uninstall_leaves_settings_json_byte_identical_to_its_pre_install_state`, which
  seeds a file with tabs and a key order no serializer would reproduce.
- [x] `PRIVACY.md` and the README accurately describe the shipped behaviour — including that
  the updater does not exist, which is a change to both.

## What is left, and who has to do it

Three things need the project owner rather than the code, and are listed here so they are not
mistaken for done:

1. **Set the release secrets** on `zaghaghi/toolog` (`MACOS_CERT_P12_BASE64`,
   `MACOS_CERT_PASSWORD`, `APPLE_AUTH_KEY_P8_BASE64`, `APPLE_AUTH_KEY_ID`,
   `APPLE_AUTH_KEY_ISSUER_ID`, `APPLE_SIGNING_IDENTITY`), then push the `v1.0.0` tag.
2. **Publish the tap**: `just cask` against the released `.dmg`, then copy
   `packaging/homebrew/toolog.rb` to `Casks/toolog.rb` in `zaghaghi/homebrew-tap`.
3. **Open the notarized build on a Mac that has never seen this certificate**, which is the
   only way to demonstrate the no-Gatekeeper-warning criterion rather than design for it.
