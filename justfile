# toolog — task runner
# `just` with no arguments lists available recipes.

# The bundler is a build tool, so it is pinned: a `.app` built here and a `.app`
# built in CI must come from the same one. `just bundle` installs it if missing.
tauri_cli := "2.11.4"

# The Developer ID that signs a release (task 8.2). Tauri reads this exact
# variable name, so exporting it in the environment works identically. Empty
# means an unsigned bundle, which is what a machine without the certificate
# gets — and it still builds.
export APPLE_SIGNING_IDENTITY := env("APPLE_SIGNING_IDENTITY", "")

default:
    @just --list

# Install the frontend's dependencies. Needed once, and after package.json moves.
ui-install:
    npm --prefix ui ci

# Type-check and bundle the window into ui/dist (Phase 5.1).
ui:
    npm --prefix ui run build

# Type-check the frontend without bundling.
ui-check:
    npm --prefix ui run check

# The frontend's tests.
ui-test:
    npm --prefix ui test

# Build the workspace (debug). The window is embedded, so it is built first.
build: ui
    cargo build --workspace --all-targets

# Build optimized.
release: ui
    cargo build --workspace --release

# Run the test suite.
test:
    cargo test --workspace --all-targets

# Format all code.
fmt:
    cargo fmt --all

# Everything CI checks, in CI's order. Run before pushing.
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Format, lint and test in one pass — Rust and the frontend.
check: fmt lint ui-check ui-test test

# Run the application. The window is compiled in, so it is bundled first.
run *ARGS: ui
    cargo run --bin toolog -- {{ARGS}}

# Report the state of the Claude Code integration. Read-only; `--fix` mutates.
doctor *ARGS:
    cargo run --bin toolog -- doctor {{ARGS}}

# Import existing history from ~/.claude/projects.
backfill *ARGS:
    cargo run --bin toolog -- backfill {{ARGS}}

# Reconcile the two ingestion lanes (ADR-0009).
verify *ARGS:
    cargo run --bin toolog -- verify {{ARGS}}

# Regenerate ui/src/bindings.ts from the Rust command surface.
bindings:
    cargo test -p toolog-app bindings

# Install or remove the login agent that keeps capture running.
agent action="status":
    cargo run --bin toolog -- agent {{action}}

# Install the pinned Tauri CLI, unless it is already the pinned version.
tauri-cli:
    #!/usr/bin/env bash
    set -euo pipefail
    have=$(cargo tauri --version 2>/dev/null | awk '{print $2}' || true)
    if [ "$have" != "{{tauri_cli}}" ]; then
        echo "installing tauri-cli {{tauri_cli}} (found: ${have:-none})"
        cargo install tauri-cli --version {{tauri_cli}} --locked
    fi

# Build the distributable universal .app and .dmg (task 8.1).
bundle: ui tauri-cli
    #!/usr/bin/env bash
    set -euo pipefail
    # Universal because a Homebrew cask ships one artifact and both
    # architectures have to run it. Signed only if APPLE_SIGNING_IDENTITY is
    # set; without it you still get a bundle, it just carries an ad-hoc
    # signature that Gatekeeper refuses.
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
    if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
        echo "signing as: $APPLE_SIGNING_IDENTITY"
    else
        echo "note: APPLE_SIGNING_IDENTITY is unset — the bundle will be unsigned."
    fi
    cargo tauri build --target universal-apple-darwin
    just verify-bundle

# Check a built bundle the way a user's Mac will (task 8.2).
verify-bundle:
    #!/usr/bin/env bash
    set -euo pipefail
    # Four separate questions, because each fails on its own and three of them
    # fail quietly: is it universal, is it signed by a Developer ID under the
    # hardened runtime, does it ask for entitlements, has it been through Apple.
    app=target/universal-apple-darwin/release/bundle/macos/toolog.app
    dmg=$(ls target/universal-apple-darwin/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)
    [ -d "$app" ] || { echo "no bundle at $app — run \`just bundle\` first"; exit 1; }

    echo "== architectures =="
    lipo -archs "$app/Contents/MacOS/toolog"

    echo "== linked libraries (ADR-0008: no network stack, task 13.4) =="
    # Phase 13 links llama.cpp, which has a `LLAMA_CURL` option for its own
    # `--hf-repo` downloader that has defaulted ON in some versions. A C library
    # linked that way is invisible to the egress test, which reads Cargo
    # manifests. Phase 8's lesson was that a config option is not a guarantee;
    # the binary is. So this asks the artifact.
    otool -L "$app/Contents/MacOS/toolog" | tail -n +2 | awk '{print "   ", $1}'
    if otool -L "$app/Contents/MacOS/toolog" | grep -qiE 'libcurl|libssl|libcrypto'; then
        echo "FAIL: the shipped binary links a network or TLS library."
        echo "      ADR-0008 says nothing leaves this machine, and llama.cpp"
        echo "      must be built with -DLLAMA_CURL=OFF."
        exit 1
    fi
    echo "    ok: no libcurl, no TLS library"

    echo "== deployment target (task 13.18) =="
    # The floor moved to 11.0 for llama.cpp's std::filesystem. Both halves of
    # the universal binary have to carry it, and it has to match the value
    # `minimumSystemVersion` puts in the installer — a .dmg that installs on a
    # Mac the binary cannot run on is worse than one that refuses.
    want=$(/usr/bin/plutil -extract bundle.macOS.minimumSystemVersion raw \
        -o - crates/toolog-app/tauri.conf.json 2>/dev/null || echo "?")
    for arch in arm64 x86_64; do
        got=$(otool -l -arch "$arch" "$app/Contents/MacOS/toolog" 2>/dev/null \
            | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')
        echo "    $arch minos ${got:-none} (config says $want)"
        [ "$got" = "$want" ] || { echo "FAIL: $arch was built for ${got:-nothing}, not $want"; exit 1; }
    done

    echo "== signature =="
    codesign -dv --verbose=4 "$app" 2>&1 | grep -E "Authority=|TeamIdentifier=|flags=" || true
    codesign --verify --strict --deep --verbose=2 "$app" 2>&1 | tail -2 || true

    echo "== entitlements (ADR-0008: expected to be an empty dict) =="
    codesign -d --entitlements - "$app" 2>/dev/null | tail -1 || true

    echo "== Gatekeeper =="
    # The question a user's first launch actually asks. "rejected" here on a
    # machine that built the app usually means notarization has not run yet.
    spctl --assess --type execute --verbose=2 "$app" 2>&1 | tail -2 || true
    if [ -n "$dmg" ]; then
        echo "== dmg =="
        ls -lh "$dmg" | awk '{print $9, $5}'
        xcrun stapler validate "$dmg" 2>&1 | tail -1 || true
    fi

# Notarize and staple the built .dmg (task 8.2).
notarize:
    #!/usr/bin/env bash
    set -euo pipefail
    # Needs an App Store Connect API key in the environment:
    #   APPLE_API_KEY_PATH  the .p8 file
    #   APPLE_API_KEY       its key id
    #   APPLE_API_ISSUER    the issuer uuid
    #
    # Tauri notarizes and staples the .app, but only *signs* the .dmg it puts
    # the app inside — so a downloaded .dmg fails Gatekeeper even though the
    # app within it would pass. The container needs notarizing in its own right.
    : "${APPLE_API_KEY_PATH:?set APPLE_API_KEY_PATH to the .p8 file}"
    : "${APPLE_API_KEY:?set APPLE_API_KEY to the key id}"
    : "${APPLE_API_ISSUER:?set APPLE_API_ISSUER to the issuer uuid}"
    dmg=$(ls target/universal-apple-darwin/release/bundle/dmg/*.dmg | head -1)
    echo "submitting $dmg"
    xcrun notarytool submit "$dmg" \
        --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER" \
        --wait
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    just verify-bundle

# SHA-256 sums for the release artifacts (task 8.4).
checksums:
    #!/usr/bin/env bash
    set -euo pipefail
    cd target/universal-apple-darwin/release/bundle/dmg
    shasum -a 256 *.dmg | tee SHA256SUMS
    echo
    echo "wrote $(pwd)/SHA256SUMS"

# Fill in the cask's version and checksum (task 8.3).
cask version="":
    #!/usr/bin/env bash
    set -euo pipefail
    # With no argument, reads the locally built .dmg — useful for testing the
    # cask. With a version ("just cask 1.0.0"), reads SHA256SUMS from that
    # GitHub release, which is the only checksum a user will ever download. A
    # local build and a CI build are not byte-identical, so publishing the
    # local one would hand everybody a cask that refuses to install.
    file=packaging/homebrew/toolog.rb
    if [ -n "{{version}}" ]; then
        v="{{version}}"
        url="https://github.com/zaghaghi/toolog/releases/download/v$v/SHA256SUMS"
        echo "reading $url"
        sha=$(curl -fsSL "$url" | awk '/universal\.dmg$/{print $1; exit}')
        [ -n "$sha" ] || { echo "no universal .dmg line in that release's SHA256SUMS"; exit 1; }
    else
        dmg=$(ls target/universal-apple-darwin/release/bundle/dmg/*.dmg | head -1)
        v=$(basename "$dmg" | sed -E 's/^toolog_(.+)_universal\.dmg$/\1/')
        sha=$(shasum -a 256 "$dmg" | awk '{print $1}')
        echo "reading the local build: $dmg"
        echo "note: a local build is not byte-identical to the released one."
        echo "      Run \`just cask $v\` once the release exists, before publishing."
    fi
    /usr/bin/sed -i '' -E "s/^  version \".*\"$/  version \"$v\"/" "$file"
    /usr/bin/sed -i '' -E "s/^  sha256 .*$/  sha256 \"$sha\"/" "$file"
    grep -E '^  (version|sha256) ' "$file"

# Lint the Homebrew cask the way Homebrew will (task 8.3).
cask-check:
    #!/usr/bin/env bash
    set -euo pipefail
    # The cask cops only run on a file that is inside a tap: linting the file
    # where it lives reports Homebrew's own Sorbet and frozen-string-literal
    # rules, which do not apply to casks, and stays silent about the ones that
    # do. So the file is staged into a throwaway tap and removed again.
    tap="$(brew --repository)/Library/Taps/toolog-caskcheck/homebrew-tmp"
    trap 'rm -rf "$(brew --repository)/Library/Taps/toolog-caskcheck"' EXIT
    mkdir -p "$tap/Casks"
    cp packaging/homebrew/toolog.rb "$tap/Casks/toolog.rb"
    # `readall` first, because it is the one that would have caught the v1.1.0
    # cask: `brew style` is RuboCop and reported no offenses on a stanza
    # (`depends_on macos: :catalina`) that `brew tap` then refused to load at
    # all. It also evaluates for every platform, not just this machine's.
    brew readall toolog-caskcheck/tmp
    brew style --cask toolog-caskcheck/tmp
    brew info --cask toolog-caskcheck/tmp/toolog | head -12

# Remove build artifacts.
clean:
    cargo clean
