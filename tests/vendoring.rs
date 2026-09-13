//! The vendored tree is upstream's, unmodified.
//!
//! `schema/s2-json/` is a verbatim redistribution of
//! [flexiblepower/s2-json](https://github.com/flexiblepower/s2-json) under the Apache
//! License 2.0. Two things follow, and both are easy to break by accident:
//!
//! * **The licence must travel with the files, unedited.** Apache-2.0 §4(a) requires
//!   giving recipients a copy of the licence, and §4(b) requires modified files to carry
//!   prominent notices. Editing the licence itself is neither — it is a violation.
//! * **The `APPENDIX` at the end of the licence is a template, not a defect.** Every
//!   correct copy of Apache-2.0 ends with `Copyright [yyyy] [name of copyright owner]` in
//!   its "How to apply" section, placeholders and all. It looks unfinished, and the
//!   natural reaction on first seeing it is to fill it in — which would corrupt the
//!   licence of a project that is not ours. This test exists because that reaction is
//!   reasonable and wrong.
//!
//! Disclosure is by file placement, which is what crates that vendor third-party material
//! actually do: `ring` ships a `LICENSE-BoringSSL`, `webpki-roots` sets the vendored data's
//! licence as its own. Here the licence sits beside the files it covers, and
//! `schema/s2-json/README.md` says whose they are — so there is nothing for the root
//! README to repeat.
//!
//! CI re-vendors from upstream and diffs, which catches tampering too. This runs offline,
//! so it catches it at `cargo test` rather than at the end of a pipeline.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()))
}

/// Ignore leading and trailing blank lines and line endings; compare the rest exactly.
fn normalize(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines[start..end].to_vec()
}

#[test]
fn the_vendored_licence_is_a_complete_apache_licence() {
    // Only the *redistributed* copy is constrained: it is upstream's file and must go out
    // exactly as it came in. `LICENSE-APACHE` at the repository root is this project's
    // own, and what it says is this project's business.
    let licence = read("schema/s2-json/LICENSE");
    let lines = normalize(&licence);
    assert_eq!(
        lines.len(),
        201,
        "the canonical Apache-2.0 text is 201 lines"
    );
    for marker in [
        "                                 Apache License",
        "                           Version 2.0, January 2004",
        "   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
        "   END OF TERMS AND CONDITIONS",
    ] {
        assert!(
            lines.contains(&marker),
            "schema/s2-json/LICENSE is missing {marker:?}; it is not the Apache-2.0 text"
        );
    }
    // All nine numbered sections.
    for n in 1..=9 {
        assert!(
            lines.iter().any(|l| l.starts_with(&format!("   {n}. "))),
            "schema/s2-json/LICENSE has no section {n}"
        );
    }
}

/// Upstream's licence goes out exactly as it came in, appendix included.
///
/// The `APPENDIX` reads `Copyright [yyyy] [name of copyright owner]` and looks like an
/// unfinished form, so the natural reaction is to fill it in. On a file we are
/// redistributing under Apache-2.0 §4(a), that would be editing somebody else's licence.
#[test]
fn the_vendored_appendix_is_upstreams_and_untouched() {
    let licence = read("schema/s2-json/LICENSE");
    assert!(
        licence.contains("APPENDIX: How to apply the Apache License to your work."),
        "schema/s2-json/LICENSE has no appendix: it is not a complete Apache-2.0 licence"
    );
    assert!(
        licence.contains("Copyright [yyyy] [name of copyright owner]"),
        "schema/s2-json/LICENSE's appendix has been edited.\n\
         That file is upstream's, redistributed verbatim; re-run \
         `cargo xtask vendor-specs` to restore it."
    );
}

/// Every `erratum E<n>` the source cites must exist on the published errata page.
///
/// The code says things like "(erratum E17)" to explain a decision that would otherwise
/// look arbitrary. Those citations are only worth anything if a reader can follow them —
/// and the page they point at is a different file from the code that cites it, so nothing
/// but a test keeps the two in step.
#[test]
fn every_erratum_the_code_cites_is_published() {
    let mut cited: Vec<u32> = Vec::new();
    let mut stack = vec![root().join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the source tree") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            for (index, _) in text.match_indices("erratum E") {
                let digits: String = text[index + "erratum E".len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if let Ok(n) = digits.parse::<u32>() {
                    cited.push(n);
                }
            }
        }
    }
    cited.sort_unstable();
    cited.dedup();
    assert!(!cited.is_empty(), "the scan found nothing; it is broken");

    let page = read("site/content/docs/errata.md");
    for n in cited {
        assert!(
            page.contains(&format!("| E{n} |")),
            "the source cites erratum E{n}, which is not on the published errata page"
        );
    }
}

#[test]
fn our_own_licences_carry_a_copyright_holder() {
    // Both of ours do name one. This only checks that neither still has an unfilled
    // placeholder where a holder is meant to go.
    for path in ["LICENSE-MIT", "LICENSE-APACHE"] {
        let text = read(path);
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("Copyright"))
            .unwrap_or_else(|| panic!("{path} carries no copyright line"));
        assert!(
            !line.contains("[name of copyright owner]") && !line.contains("<copyright holders>"),
            "{path} still has a placeholder copyright line: {line:?}"
        );
    }
}

#[test]
fn the_vendored_tree_says_whose_it_is_and_where_it_came_from() {
    // Apache-2.0 does not require a README, but a directory of somebody else's
    // differently-licensed files in the middle of an MIT-or-Apache crate needs to say so
    // where a reader — or a licence scanner — will see it.
    let readme = read("schema/s2-json/README.md");
    assert!(
        readme.contains("FlexiblePower Alliance Network"),
        "{readme}"
    );
    assert!(readme.contains("Apache License"), "{readme}");
    assert!(
        readme.contains("flexiblepower/s2-json"),
        "the README must name the upstream repository"
    );
    // The revision, so "this is upstream's" is a checkable claim rather than an assertion.
    let revision = readme
        .lines()
        .find_map(|line| line.strip_prefix("Vendored from revision `"))
        .and_then(|rest| rest.split('`').next())
        .expect("the README must record the upstream revision");
    assert_eq!(revision.len(), 40, "a full git object id, not {revision:?}");
    assert!(
        revision.bytes().all(|b| b.is_ascii_hexdigit()),
        "{revision:?} is not a git object id"
    );
}

#[test]
fn every_vendored_file_is_one_the_generator_would_produce() {
    // A stray file in a verbatim redistribution is a file nobody can account for: not
    // upstream's, not generated, and not covered by anything either licence says.
    let dir = root().join("schema/s2-json");
    let mut unexpected = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("the vendored directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(&dir)
                .expect("inside the vendored directory")
                .to_string_lossy()
                .replace('\\', "/");
            let is_schema = path.extension().is_some_and(|e| e == "json")
                && ["messages/", "schemas/", "profiles/"]
                    .iter()
                    .any(|dir| relative.starts_with(dir));
            let expected = relative == "LICENSE" || relative == "README.md" || is_schema;
            if !expected {
                unexpected.push(relative);
            }
        }
    }
    assert!(
        unexpected.is_empty(),
        "unaccounted-for files under schema/s2-json/: {unexpected:?}\n\
         Everything there is either upstream's or written by `cargo xtask vendor-specs`."
    );
}
