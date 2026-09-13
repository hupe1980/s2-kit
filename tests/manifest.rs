//! The feature graph means what the documentation says it means.
//!
//! Cargo features fail silently: a `?`-gated feature whose optional dependency is never
//! activated is not an error, not a warning and not visible in `cargo tree`. It is inert,
//! and the first sign of it is a runtime fault on a machine that is not the one that built
//! the crate. These are the invariants of this manifest nothing else can check (D52).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;

fn manifest() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The named sections of the manifest, keyed by header (`features`, `dependencies`, …).
fn sections(text: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            current = trimmed.trim_start_matches('[').trim_end_matches(']').into();
            continue;
        }
        out.entry(current.clone()).or_default().push_str(line);
        out.entry(current.clone()).or_default().push('\n');
    }
    out
}

/// Every `name = [ … ]` entry of `[features]`, with the list flattened onto one line.
///
/// Comments are stripped first: this manifest explains most of its features in a comment
/// above them, and a comment that names a feature must not read as the feature itself.
fn features(text: &str) -> BTreeMap<String, Vec<String>> {
    let section = sections(text)
        .remove("features")
        .expect("the manifest has a [features] section");
    let stripped: String = section
        .lines()
        .map(|l| l.split_once('#').map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = BTreeMap::new();
    let mut rest = stripped.as_str();
    while let Some((head, tail)) = rest.split_once('=') {
        let name = head.rsplit('\n').next().unwrap_or(head).trim().to_string();
        let Some((list, after)) = tail
            .trim_start()
            .strip_prefix('[')
            .and_then(|t| t.split_once(']'))
        else {
            break;
        };
        let entries = list
            .split(',')
            .map(|e| e.trim().trim_matches('"').to_string())
            .filter(|e| !e.is_empty())
            .collect();
        if !name.is_empty() {
            out.insert(name, entries);
        }
        rest = after;
    }
    out
}

#[test]
fn every_feature_that_brings_rustls_in_also_activates_it() {
    let text = manifest();
    let features = features(&text);

    // The two provider features are `?`-gated, so they only reach a `rustls` this crate
    // activated itself. Anything that can end up with `rustls` in the graph must
    // therefore say `dep:rustls`, or the provider selection silently does nothing.
    for gated in ["tls-ring", "tls-aws-lc-rs"] {
        let list = features
            .get(gated)
            .unwrap_or_else(|| panic!("`{gated}` is a feature"));
        assert!(
            list.iter().any(|e| e.starts_with("rustls?/")),
            "`{gated}` is expected to be `?`-gated on rustls: {list:?}"
        );
    }

    for (name, list) in &features {
        // `tokio-tungstenite`'s TLS support *is* rustls, so a feature that turns it on
        // is a feature that brings rustls in.
        let brings_rustls = list
            .iter()
            .any(|e| e.starts_with("tokio-tungstenite/") && e.contains("rustls"));
        if !brings_rustls {
            continue;
        }
        assert!(
            list.contains(&"dep:rustls".to_string()),
            "`{name}` puts rustls in the dependency graph through tokio-tungstenite but \
             does not activate `dep:rustls`, so `tls-ring`/`tls-aws-lc-rs` cannot reach \
             it and the build has no crypto provider: {list:?}"
        );
    }
}

#[test]
fn the_default_feature_set_pulls_no_tls_at_all() {
    let text = manifest();
    let features = features(&text);
    let default = features.get("default").expect("a `default` feature");

    // `tls-ring` is in `default` so that *when* TLS is compiled in there is a provider,
    // not so that every consumer links one. Nothing in `default` may drag rustls in.
    for entry in default {
        assert!(
            !matches!(
                entry.as_str(),
                "tokio" | "connect-client" | "connect-server"
            ),
            "`default` must not enable `{entry}`: a plain `s2-kit = \"…\"` dependency is \
             expected to pull no TLS stack, and `cargo tree` is the test that proves it"
        );
    }
    assert!(
        default.contains(&"tls-ring".to_string()),
        "a build that does compile TLS in should have a provider without asking: {default:?}"
    );
}

#[test]
fn tokio_tungstenite_brings_no_second_root_store() {
    let text = manifest();
    let deps = sections(&text)
        .remove("dependencies")
        .expect("a [dependencies] section");
    let line = deps
        .lines()
        .find(|l| l.trim_start().starts_with("tokio-tungstenite"))
        .expect("tokio-tungstenite is a dependency");

    // This crate always supplies its own `Connector::Rustls`, so the library never builds
    // a `ClientConfig` and never needs roots. Asking for them would ship a second copy of
    // Mozilla's list — `webpki-roots` 0.26, itself a shim over 1.0 — that nothing reads,
    // *and* re-open the implicit-provider path this crate exists to avoid.
    assert!(
        !line.contains("rustls-tls-webpki-roots") && !line.contains("rustls-tls-native-roots"),
        "tokio-tungstenite should not carry a root store: {line}"
    );
}
