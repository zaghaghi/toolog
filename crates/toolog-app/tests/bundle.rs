//! The packaging configuration, asserted rather than trusted (Phase 8).
//!
//! Everything here is a property of files that are read by `codesign`, `tauri
//! build` and macOS itself — none of which run during `cargo test`, and two of
//! which fail in ways that are quiet or misleading. So the properties are
//! pinned here, where breaking one costs a red test rather than a bad release.

use std::path::{Path, PathBuf};

fn app_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(name: &str) -> String {
    let path = app_crate().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The body of an XML file with every comment removed.
fn without_comments(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let end = after
            .find("-->")
            .unwrap_or_else(|| panic!("an XML comment was opened and never closed"));
        rest = &after[end + 3..];
    }
    out.push_str(rest);
    out
}

/// ADR-0008's posture, as the signature will carry it.
///
/// An entitlement is a relaxation of the hardened runtime. The claim in
/// `PRIVACY.md` is that toolog asks for none, and this is what stops that
/// becoming false by someone adding "just one".
#[test]
fn the_shipped_app_asks_for_no_entitlements() {
    let body = without_comments(&read("entitlements.plist"));

    assert!(
        !body.contains("<key>"),
        "entitlements.plist declares something. Every entitlement hands back a \
         protection the hardened runtime would otherwise enforce, and PRIVACY.md \
         says toolog asks for none — so adding one means changing that claim \
         too, deliberately:\n{body}"
    );

    let dict = body
        .split_once("<dict>")
        .map(|(_, after)| after)
        .and_then(|after| after.split_once("</dict>"))
        .map(|(inside, _)| inside.trim())
        .expect("entitlements.plist has a <dict> element");
    assert!(dict.is_empty(), "the dict is not empty: {dict:?}");
}

/// The trap that cost a signing run: `plutil` accepts what `codesign` rejects.
///
/// XML forbids two consecutive hyphens inside a comment. `plutil -lint` reports
/// such a file as valid; `codesign` fails with `AMFIUnserializeXML: syntax
/// error near line N` and signs the app **without** the entitlements rather
/// than stopping. Writing a command line in a comment is the natural way to
/// hit this, because every long option starts with two hyphens.
#[test]
fn no_comment_in_a_plist_contains_a_double_hyphen() {
    for name in ["entitlements.plist", "Info.plist"] {
        let xml = read(name);
        let mut rest = xml.as_str();
        while let Some(start) = rest.find("<!--") {
            let after = &rest[start + 4..];
            let end = after.find("-->").expect("an unclosed comment");
            let comment = &after[..end];
            assert!(
                !comment.contains("--"),
                "{name}: a comment contains a double hyphen, which XML forbids and \
                 codesign's entitlement parser rejects while plutil calls the file \
                 valid:\n{comment}"
            );
            rest = &after[end + 3..];
        }
    }
}

/// A menu-bar app must say so before AppKit decides otherwise.
///
/// `app.rs` also sets `ActivationPolicy::Accessory`, but that runs after the
/// process is up, so without this key the Dock icon appears and then vanishes.
#[test]
fn the_bundle_declares_itself_a_menu_bar_app() {
    let plist = without_comments(&read("Info.plist"));
    let ui_element = plist
        .split_once("<key>LSUIElement</key>")
        .map(|(_, after)| after.trim_start())
        .expect("Info.plist sets LSUIElement");
    assert!(
        ui_element.starts_with("<true/>"),
        "LSUIElement must be true, or the app takes a Dock icon and an \
         app-switcher entry it then has to remove at runtime"
    );
}

/// The `.icns` is the only icon a macOS bundle actually shows.
///
/// The PNG list is enough for the tray and the window. Leaving the `.icns` out
/// bundles fine and produces an app with a blank generic icon in Finder, the
/// Dock and the installer window — a failure nothing in the build reports.
#[test]
fn the_macos_bundle_has_an_icns_and_signs_with_the_entitlements_file() {
    let conf: serde_json::Value =
        serde_json::from_str(&read("tauri.conf.json")).expect("tauri.conf.json is JSON");

    let icons: Vec<&str> = conf["bundle"]["icon"]
        .as_array()
        .expect("bundle.icon is a list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        icons
            .iter()
            .filter_map(|i| Path::new(i).extension())
            .any(|e| e.eq_ignore_ascii_case("icns")),
        "no .icns in {icons:?}; macOS would show a generic icon"
    );

    for icon in &icons {
        let path = app_crate().join(icon);
        assert!(path.is_file(), "{} is listed but missing", path.display());
    }

    assert_eq!(
        conf["bundle"]["macOS"]["entitlements"].as_str(),
        Some("entitlements.plist"),
        "the bundler must be told to sign with the entitlements file, or the \
         empty-entitlements claim is about a file nothing reads"
    );
}

/// One source of truth for the version.
///
/// Tauri falls back to the crate's `Cargo.toml` when `version` is absent from
/// the config. Setting it in both is how a `.dmg` comes to be named after a
/// version the binary does not report.
#[test]
fn the_bundle_version_comes_from_cargo_and_nowhere_else() {
    let conf: serde_json::Value =
        serde_json::from_str(&read("tauri.conf.json")).expect("tauri.conf.json is JSON");
    assert!(
        conf.get("version").is_none(),
        "tauri.conf.json pins a version of its own; it would silently win over \
         Cargo.toml and could drift from `toolog --version`"
    );
    assert_eq!(env!("CARGO_PKG_VERSION"), "1.0.0");
}
