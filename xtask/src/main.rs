//! Repository automation for `s2-kit`.
//!
//! ```text
//! cargo xtask vendor-specs        copy the official specifications into schema/
//! cargo xtask gen-schema-table    regenerate src/schema/table.rs from schema/s2-json
//! cargo xtask check-docs          check the model against the standard's own field docs
//! cargo xtask gen-model-reference write the data-model reference pages of the site
//! cargo xtask render-rules        write the rule catalogue page of the site
//! cargo xtask gen-conformance     write the conformance statement page of the site
//! cargo xtask gen-site-examples   turn the website's code blocks into doctests
//! cargo xtask all                 everything above, in order
//! ```
//!
//! `all` skips `gen-model-reference` and `check-docs` when `specs/s2-documentation` is
//! absent, so a contributor without it can still regenerate the rest. CI therefore runs
//! those two by name, where a missing clone is an error rather than a shrug.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

mod site_examples;

fn main() -> Result<()> {
    let task = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let root = repo_root()?;
    match task.as_str() {
        "vendor-specs" => vendor_specs(&root),
        "gen-schema-table" => gen_schema_table(&root),
        "check-docs" => check_docs(&root),
        "gen-model-reference" => gen_model_reference(&root),
        "render-rules" => render_rules(&root),
        "gen-conformance" => gen_conformance(&root),
        "gen-site-examples" => gen_site_examples(&root),
        "all" => {
            gen_schema_table(&root)?;
            render_rules(&root)?;
            gen_conformance(&root)?;
            gen_site_examples(&root)?;
            gen_model_reference(&root).or_else(skip_if_no_specs)?;
            check_docs(&root).or_else(skip_if_no_specs)
        }
        other => bail!(
            "unknown task {other:?}; try vendor-specs, gen-schema-table, render-rules, gen-conformance, gen-site-examples, check-docs, gen-model-reference, all"
        ),
    }
}

/// Turn the website's runnable code blocks into doctests.
///
/// The README is already checked this way (`lib.rs` includes it under `cfg(doctest)`);
/// the guides were not, and a guide whose first snippet does not compile is worse than no
/// guide at all.
fn gen_site_examples(root: &Path) -> Result<()> {
    let count = site_examples::generate(root).context("writing src/site_doctests.rs")?;
    println!("wrote src/site_doctests.rs ({count} runnable blocks from the site)");
    Ok(())
}

fn skip_if_no_specs(e: anyhow::Error) -> Result<()> {
    if e.to_string().contains("specs/") {
        eprintln!("skipping: {e}");
        Ok(())
    } else {
        Err(e)
    }
}

fn repo_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Ok(manifest
        .parent()
        .context("xtask has no parent directory")?
        .to_path_buf())
}

// ---------------------------------------------------------------------------
// vendor-specs
// ---------------------------------------------------------------------------

/// Copies the official schemas into `schema/s2-json`, licence and all.
///
/// Everything under `schema/s2-json/` is a verbatim redistribution of
/// `flexiblepower/s2-json`, which is Apache-2.0. Clause 4(a) of that licence requires
/// giving recipients a copy of it, so the `LICENSE` is copied with the files rather than
/// left to be remembered — a vendored tree whose licence is maintained by hand is a
/// vendored tree whose licence goes stale.
///
/// The `profiles/v0.0.2-beta` subtree is the one place the two tagged versions differ, and
/// it comes from the `v0.0.2-beta` tag of the same repository — so it is upstream material
/// too, and covered by the same licence. It is taken from git rather than from the
/// working tree, because a checkout can only be at one tag at a time.
fn vendor_specs(root: &Path) -> Result<()> {
    let specs = root.join("specs/s2-json");
    if !specs.exists() {
        bail!("specs/s2-json is not present; clone it there first (it is gitignored)");
    }
    let target = root.join("schema/s2-json");

    let mut copied = 0usize;
    for dir in ["messages", "schemas"] {
        let from = specs.join(dir);
        let to = target.join(dir);
        std::fs::create_dir_all(&to)?;
        for entry in std::fs::read_dir(&from)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|e| e == "json") {
                std::fs::copy(entry.path(), to.join(entry.file_name()))?;
                copied += 1;
            }
        }
    }

    // The licence travels with the files it covers.
    std::fs::copy(specs.join("LICENSE"), target.join("LICENSE"))
        .context("copying the upstream LICENSE, which Apache-2.0 §4(a) requires")?;

    // The beta profile, from the tag rather than the working tree.
    let beta = target.join("profiles/v0.0.2-beta");
    std::fs::create_dir_all(&beta)?;
    for file in BETA_PROFILE_FILES {
        let path = format!("v0.0.2-beta:s2-json-schema/messages/{file}");
        let output = std::process::Command::new("git")
            .args(["-C", &specs.to_string_lossy(), "show", &path])
            .output()
            .context("running git to read the v0.0.2-beta tag")?;
        if !output.status.success() {
            bail!(
                "specs/s2-json has no v0.0.2-beta tag (a shallow clone will not do): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        std::fs::write(beta.join(file), &output.stdout)?;
        copied += 1;
    }

    let revision = git_revision(&specs).unwrap_or_else(|| "unknown".to_string());
    write_schema_readme(&target, &revision)?;
    println!("vendored {copied} files into schema/s2-json from specs/s2-json at {revision}");
    Ok(())
}

/// The files that differ between the two tagged versions.
///
/// One, today. If a third version ever moves a second field,
/// this list is where that becomes visible rather than a surprise.
const BETA_PROFILE_FILES: &[&str] = &["DDBC.SystemDescription.schema.json"];

fn git_revision(repo: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Says what is vendored, from where, and under what licence.
///
/// A directory of somebody else's Apache-2.0 files in the middle of an MIT-or-Apache crate
/// is exactly the thing a reader — or a licence scanner — needs told explicitly.
fn write_schema_readme(target: &Path, revision: &str) -> Result<()> {
    let readme = format!(
        concat!(
            "# Vendored S2 JSON schemas\n\n",
            "@generated by `cargo xtask vendor-specs` — do not edit.\n\n",
            "Everything in this directory is a verbatim copy of\n",
            "[flexiblepower/s2-json](https://github.com/flexiblepower/s2-json),\n",
            "© FlexiblePower Alliance Network, distributed under the **Apache License\n",
            "2.0** — see `LICENSE`, which is upstream's own copy. It is **not** covered\n",
            "by this crate's `MIT OR Apache-2.0`.\n\n",
            "Vendored from revision `{}`.\n\n",
            "| Path | What |\n|---|---|\n",
            "| `messages/` | The 36 message schemas of S2 JSON v1.0.0 |\n",
            "| `schemas/` | The component schemas they reference |\n",
            "| `profiles/v0.0.2-beta/` | The files that differ at the `v0.0.2-beta` tag — one, today |\n",
            "| `LICENSE` | Upstream's licence, copied because Apache-2.0 §4(a) requires it |\n\n",
            "### Why the licence is copied rather than linked\n\n",
            "Because Apache-2.0 makes it a condition of redistributing the files at all:\n\n",
            "> **4. Redistribution.** You may reproduce and distribute copies of the Work\n",
            "> … provided that You meet the following conditions:\n",
            "> **(a)** You must *give* any other recipients of the Work … *a copy of this\n",
            "> License*.\n\n",
            "A link in a README is a pointer, not a copy, and it fails in both directions\n",
            "that matter. Someone who downloads the `.crate` from crates.io receives the\n",
            "schemas in the tarball — if the licence is not in there with them, they have\n",
            "received the Work without its licence. And a URL stops resolving when a\n",
            "repository is renamed, moved or deleted, while the obligation does not lapse\n",
            "with it.\n\n",
            "It costs about 4 KB compressed, a little over 1% of the published crate.\n\n",
            "### About that `[yyyy]` in `LICENSE`\n\n",
            "`LICENSE` ends with an `APPENDIX: How to apply the Apache License to your\n",
            "work`, whose boilerplate reads `Copyright [yyyy] [name of copyright owner]`.\n",
            "Leave it. That file is **upstream's**, redistributed byte for byte under\n",
            "Apache-2.0 §4(a); editing it would be editing somebody else's licence, and\n",
            "`tests/vendoring.rs` fails if anyone does. Upstream's copyright holder is\n",
            "named above, which is the part that is ours to write.\n\n",
            "## Why they are committed rather than fetched\n\n",
            "* The crate must build and test **offline**. docs.rs has no network, and\n",
            "  neither do most CI sandboxes.\n",
            "* A fetched copy makes the tests mean something different from one day to\n",
            "  the next, silently, when upstream changes. A committed one makes that a\n",
            "  reviewable diff.\n",
            "* This crate's central claim is that its hand-written model is *proven*\n",
            "  against the official schemas. Shipping them in the published `.crate`\n",
            "  makes that claim checkable by anyone who downloads it — for about 19 KiB\n",
            "  compressed, which is the whole cost.\n\n",
            "Nothing in `src/` reads them at build time: `src/schema/table.rs` is\n",
            "*generated* from them, and is a few kilobytes of names and array bounds\n",
            "rather than three hundred of JSON. They are used by\n",
            "`tests/model_matches_schema.rs`, which validates every message against them\n",
            "with a real JSON Schema validator, and by `cargo xtask gen-schema-table`.\n\n",
            "To refresh them, clone `s2-json` into `specs/` (gitignored, and it must not\n",
            "be a shallow clone — the `v0.0.2-beta` tag is read from git history) and run\n",
            "`cargo xtask vendor-specs`. CI fails if the result differs from what is\n",
            "committed.\n",
        ),
        revision
    );
    std::fs::write(target.join("README.md"), readme)?;
    Ok(())
}

fn ref_name(r: &str) -> String {
    r.rsplit('/')
        .next()
        .unwrap_or(r)
        .trim_end_matches(".schema.json")
        .to_string()
}

fn load_schemas(dir: &Path) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for sub in ["messages", "schemas"] {
        let d = dir.join(sub);
        for entry in std::fs::read_dir(&d).with_context(|| format!("reading {}", d.display()))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .trim_end_matches(".schema.json")
                    .to_string();
                let text = std::fs::read_to_string(&path)?;
                let value: Value =
                    serde_json::from_str(&text).with_context(|| format!("parsing {name}"))?;
                out.insert(name, value);
            }
        }
    }
    Ok(out)
}

/// Whether a referenced schema is an object with properties, as opposed to a scalar or
/// an enumeration.
fn is_object(schemas: &BTreeMap<String, Value>, name: &str) -> bool {
    schemas
        .get(name)
        .is_some_and(|s| s.get("properties").is_some())
}

/// The generator's own view of a type, before it becomes Rust source.
///
/// It mirrors `s2_kit::schema::TypeSpec`, but owns its strings: the generator is building
/// the very table that type is read out of, so it cannot borrow from it.
struct TypeSpec {
    name: String,
    /// `Some` when this type is a message, carrying its `message_type` constant.
    message_type: Option<String>,
    properties: Vec<Property>,
}

struct Property {
    name: String,
    required: bool,
    kind: Kind,
}

/// What a property holds, to the depth the decoder and validator need.
enum Kind {
    /// A number, a string, an enumeration — anything with no properties of its own.
    Scalar,
    /// A nested object, named by its schema.
    Object(String),
    /// An array of scalars, with the schema's `minItems` and `maxItems`.
    ScalarArray(Option<u64>, Option<u64>),
    /// An array of objects, likewise.
    ObjectArray(String, Option<u64>, Option<u64>),
}

fn gen_schema_table(root: &Path) -> Result<()> {
    let schemas = load_schemas(&root.join("schema/s2-json"))?;
    let mut specs: Vec<TypeSpec> = Vec::new();

    for (name, schema) in &schemas {
        let Some(props) = schema.get("properties").and_then(Value::as_object) else {
            continue; // a scalar or an enumeration: no property table to build
        };
        let required: BTreeSet<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        let message_type = props
            .get("message_type")
            .and_then(|m| m.get("const"))
            .and_then(Value::as_str)
            .map(str::to_string);

        let mut properties = Vec::new();
        for (prop_name, prop) in props {
            if prop_name == "message_type" {
                continue; // the discriminator is the enum tag, not a field
            }
            let kind = if let Some(r) = prop.get("$ref").and_then(Value::as_str) {
                let target = ref_name(r);
                if is_object(&schemas, &target) {
                    Kind::Object(target)
                } else {
                    Kind::Scalar
                }
            } else if prop.get("type").and_then(Value::as_str) == Some("array") {
                let min = prop.get("minItems").and_then(Value::as_u64);
                let max = prop.get("maxItems").and_then(Value::as_u64);
                let items = prop.get("items");
                match items.and_then(|i| i.get("$ref")).and_then(Value::as_str) {
                    Some(r) => {
                        let target = ref_name(r);
                        if is_object(&schemas, &target) {
                            Kind::ObjectArray(target, min, max)
                        } else {
                            Kind::ScalarArray(min, max)
                        }
                    }
                    None => Kind::ScalarArray(min, max),
                }
            } else {
                Kind::Scalar
            };
            properties.push(Property {
                name: prop_name.clone(),
                required: required.contains(prop_name.as_str()),
                kind,
            });
        }
        specs.push(TypeSpec {
            name: name.clone(),
            message_type,
            properties,
        });
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::new();
    writeln!(
        out,
        "// @generated by `cargo xtask gen-schema-table` from schema/s2-json — do not edit.\n\
         //\n\
         // The property table of S2 JSON v1.0.0: every object type, its properties in\n\
         // schema order, which are required, and the bounds on every array. It is what\n\
         // lets the lenient decoder prune unknown properties and the validator check\n\
         // cardinality without anyone transcribing a number by hand.\n"
    )?;
    writeln!(
        out,
        "use super::{{ArrayBounds, Kind, Property, TypeSpec}};\n"
    )?;

    writeln!(
        out,
        concat!(
            "/// Every object type in S2 JSON v1.0.0, sorted by name.\n",
            "///\n",
            "/// One line per property, which is how a human reads a table; rustfmt would\n",
            "/// explode each of the 550 entries across five lines and make it unreadable,\n",
            "/// so it is asked not to.\n",
            "#[rustfmt::skip]\n",
            "pub static TYPES: &[TypeSpec] = &["
        )
    )?;
    for spec in &specs {
        writeln!(out, "    TypeSpec {{")?;
        writeln!(out, "        name: {:?},", spec.name)?;
        match &spec.message_type {
            Some(m) => writeln!(out, "        message_type: Some({m:?}),")?,
            None => writeln!(out, "        message_type: None,")?,
        }
        writeln!(out, "        properties: &[")?;
        for p in &spec.properties {
            let kind = match &p.kind {
                Kind::Scalar => "Kind::Scalar".to_string(),
                Kind::Object(t) => format!("Kind::Object({t:?})"),
                Kind::ScalarArray(min, max) => {
                    format!("Kind::ScalarArray({})", bounds(*min, *max))
                }
                Kind::ObjectArray(t, min, max) => {
                    format!("Kind::ObjectArray({:?}, {})", t, bounds(*min, *max))
                }
            };
            writeln!(
                out,
                "            Property {{ name: {:?}, required: {}, kind: {} }},",
                p.name, p.required, kind
            )?;
        }
        writeln!(out, "        ],")?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;

    let path = root.join("src/schema/table.rs");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, out)?;
    println!(
        "wrote {} ({} object types, {} messages)",
        path.display(),
        specs.len(),
        specs.iter().filter(|s| s.message_type.is_some()).count()
    );
    Ok(())
}

fn bounds(min: Option<u64>, max: Option<u64>) -> String {
    format!(
        "ArrayBounds {{ min: {}, max: {} }}",
        min.map_or("None".into(), |v| format!("Some({v})")),
        max.map_or("None".into(), |v| format!("Some({v})"))
    )
}

// ---------------------------------------------------------------------------
// render-rules
// ---------------------------------------------------------------------------

/// Zola front matter: a title and a description for every generated page.
///
/// The description is not decoration. It becomes the `<meta name="description">` and the
/// OpenGraph summary, and a page without one is a page search engines and chat clients
/// summarise for you — usually with the first table row.
fn front_matter(title: &str, description: &str, weight: Option<usize>) -> String {
    // A description is rendered as plain text, in a `<meta>` tag and as the page's lede.
    // Markdown left in it shows up as literal backticks in search results.
    let description = description.replace('`', "").replace('*', "");
    let mut out = String::from("+++\n");
    let _ = writeln!(out, "title = {title:?}");
    let _ = writeln!(out, "description = {description:?}");
    if let Some(weight) = weight {
        let _ = writeln!(out, "weight = {weight}");
    }
    out.push_str("[extra]\ngenerated = true\n+++\n\n");
    out
}

/// The site's content root.
fn content(root: &Path) -> PathBuf {
    root.join("site/content")
}

/// Writes the rule catalogue page, so the document and the code cannot
/// disagree about which rules exist or how severe they are.
fn render_rules(root: &Path) -> Result<()> {
    let dir = content(root).join("reference");
    std::fs::create_dir_all(&dir)?;
    let count = s2_kit::validate::rules::RULES.len();
    let errors = s2_kit::validate::rules::RULES
        .iter()
        .filter(|r| r.severity == s2_kit::validate::Severity::Error)
        .count();
    let page = format!(
        "{}{}",
        front_matter(
            "Rule catalogue",
            &format!(
                "All {count} semantic validation rules s2-kit enforces on S2 messages — \
                 {errors} errors and {} warnings, each quoting the sentence of the \
                 standard it implements.",
                count - errors
            ),
            Some(10),
        ),
        // The generated markdown opens with its own `# Rule catalogue`; the template
        // renders the title, so drop the duplicate.
        strip_leading_h1(&s2_kit::validate::rules_markdown()),
    );
    std::fs::write(dir.join("rules.md"), &page)?;
    println!("wrote site/content/reference/rules.md ({count} rules)");
    Ok(())
}

// ---------------------------------------------------------------------------
// gen-conformance
// ---------------------------------------------------------------------------

/// Writes the conformance page: what this crate implements, derived from the crate.
///
/// A hand-written conformance statement is a promise; this one is a measurement. Every
/// table below is produced by asking the code — the message catalogue, the state table,
/// the rule catalogue, the profile difference — so the document cannot claim support for
/// something that was removed, or stay silent about something that was added.
fn gen_conformance(root: &Path) -> Result<()> {
    use s2_kit::message::MessageKind;
    use s2_kit::types::WireProfile;
    use s2_kit::types::common::{ControlType, EnergyManagementRole};
    use s2_kit::validate::state;

    let mut out = String::new();
    out.push_str(
        "# Conformance statement\n\nGenerated by `cargo xtask gen-conformance` from the crate itself — do not edit.\n\nEach table is read out of the code, so a claim here is a claim the compiler is\nholding up. What is *not* claimed is listed at the end.\n\n",
    );

    // --- Messages -----------------------------------------------------------
    writeln!(out, "## Messages\n")?;
    writeln!(
        out,
        "All {} messages of S2 JSON v1.0.0 are modelled, validated and routed.\n",
        MessageKind::ALL.len()
    )?;
    writeln!(
        out,
        "| Message | Control type | Sender | Instruction | v1.0.0 | v0.0.2-beta |"
    )?;
    writeln!(out, "|---|---|---|---|---|---|")?;
    for kind in MessageKind::ALL {
        writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} |",
            kind.as_str(),
            kind.control_type()
                .map_or_else(|| "—".to_string(), |c| format!("{c:?}")),
            match kind.sender() {
                Some(EnergyManagementRole::Cem) => "CEM",
                Some(EnergyManagementRole::Rm) => "RM",
                None => "either",
            },
            if kind.is_instruction() { "yes" } else { "—" },
            tick(kind.exists_in(WireProfile::V1_0_0)),
            tick(kind.exists_in(WireProfile::V0_0_2Beta)),
        )?;
    }

    // --- The state table ----------------------------------------------------
    writeln!(out, "\n## State of communication\n")?;
    writeln!(
        out,
        "`S2C §State of communication` as this crate enforces it: what each side may send\nin each state. Both engines refuse an outbound message outside this table at the\ncall site, and answer an inbound one with `INVALID_CONTENT`.\n"
    )?;
    let states: Vec<(String, Option<ControlType>)> =
        core::iter::once(("connected".to_string(), None))
            .chain(
                [
                    ControlType::PowerEnvelopeBasedControl,
                    ControlType::PowerProfileBasedControl,
                    ControlType::OperationModeBasedControl,
                    ControlType::FillRateBasedControl,
                    ControlType::DemandDrivenBasedControl,
                    ControlType::NotControllable,
                    ControlType::NoSelection,
                ]
                .into_iter()
                .map(|c| (format!("{c:?}"), Some(c))),
            )
            .collect();
    writeln!(out, "| State | CEM may send | RM may send |")?;
    writeln!(out, "|---|---|---|")?;
    for (name, active) in &states {
        let cem = state::allowed_kinds(*active, EnergyManagementRole::Cem);
        let rm = state::allowed_kinds(*active, EnergyManagementRole::Rm);
        writeln!(
            out,
            "| {name} | {} | {} |",
            join_kinds(&cem),
            join_kinds(&rm)
        )?;
    }

    // --- Rules --------------------------------------------------------------
    let rules = s2_kit::validate::rules::RULES;
    let errors = rules
        .iter()
        .filter(|r| r.severity == s2_kit::validate::Severity::Error)
        .count();
    writeln!(out, "\n## Semantic validation\n")?;
    writeln!(
        out,
        concat!(
            "{} rules — {} errors, {} warnings — each quoting the sentence it implements.\n",
            "The full catalogue is in the [rule reference](@/reference/rules.md); every one\n",
            "of them has a test that fires it.\n"
        ),
        rules.len(),
        errors,
        rules.len() - errors
    )?;
    let needs_context = rules.iter().filter(|r| r.needs_context).count();
    writeln!(
        out,
        "{needs_context} of them need session context — a rule that compares a message\nagainst what the peer said earlier cannot fire on a message read from a file.\n"
    )?;

    // --- Profiles -----------------------------------------------------------
    writeln!(out, "\n## Wire profiles\n")?;
    writeln!(
        out,
        "| Profile | Protocol version string | Messages |\n|---|---|---|"
    )?;
    for profile in [WireProfile::V1_0_0, WireProfile::V0_0_2Beta] {
        let count = MessageKind::ALL
            .iter()
            .filter(|k| k.exists_in(profile))
            .count();
        writeln!(
            out,
            "| `{profile:?}` | `{}` | {count} |",
            profile.version_str()
        )?;
    }
    let only_beta: Vec<MessageKind> = MessageKind::ALL
        .iter()
        .copied()
        .filter(|k| k.exists_in(WireProfile::V0_0_2Beta) && !k.exists_in(WireProfile::V1_0_0))
        .collect();
    let only_release: Vec<MessageKind> = MessageKind::ALL
        .iter()
        .copied()
        .filter(|k| k.exists_in(WireProfile::V1_0_0) && !k.exists_in(WireProfile::V0_0_2Beta))
        .collect();
    writeln!(
        out,
        "\nOnly in `v1.0.0`: {}.  \nOnly in `v0.0.2-beta`: {}.\n",
        join_kinds(&only_release),
        join_kinds(&only_beta)
    )?;

    // --- What is not claimed ------------------------------------------------
    out.push_str(concat!(
        "\n## S2 Connect\n\n",
        "Discovery, pairing, session initiation and the transport are implemented for\n",
        "both sides. `tests/connect_e2e.rs` pairs this crate's client against this\n",
        "crate's server over a real TLS connection, which exercises the one thing an\n",
        "in-memory test cannot: the leaf-certificate fingerprint the challenge response\n",
        "binds to comes out of a genuine handshake.\n\n",
        "| Part | State |\n|---|---|\n",
        "| Pairing, both sides, with the per-node rate limit and the mandatory delay | yes |\n",
        "| Session initiation, both sides, with the two-phase token commit | yes |\n",
        "| LAN trust: unverified during pairing, CA-pinned afterwards | yes |\n",
        "| DNS-SD advertise and browse, `_cem` / `_rm` subtypes | yes |\n",
        "| LAN-only operations: `/endpoint`, `/nodes`, `/preparePairing`, `/cancelPreparePairing` | yes |\n",
        "| The same-subnet check those operations require, v4 and v6 | yes |\n",
        "| Long-polling (`/waitForPairing`) | wire types only |\n",
        "| WAN endpoint registry | no |\n",
        "\n## Not claimed\n\n",
        "* **Interoperability results.** Nothing here has been run against another\n",
        "  implementation in CI yet; when it has, the matrix will live in this file\n",
        "  and say which revision of which peer it was run against. Pairing this\n",
        "  crate against itself proves self-consistency, which is not the same thing.\n",
        "* **Certification.** There is no S2 certification programme, and this\n",
        "  document is not one.\n",
        "* **Deciding whether you are a LAN or a WAN endpoint.** `router` serves only\n",
        "  the authenticated operations, so a WAN endpoint answers `404` on the rest, as\n",
        "  `S2C` recommends. `lan_router` adds them and *requires* `LocalSubnets`, so the\n",
        "  unauthenticated operations cannot be exposed without their access control. Which\n",
        "  one you mount is yours to choose; forgetting the subnet check is not.\n",
    ));

    let dir = content(root).join("reference");
    std::fs::create_dir_all(&dir)?;
    let page = format!(
        "{}{}",
        front_matter(
            "Conformance",
            "What s2-kit implements of S2 JSON and S2 Connect, generated from the crate \
             itself: the message catalogue, the state table, the wire profiles, and what \
             is explicitly not claimed.",
            Some(20),
        ),
        strip_leading_h1(&out),
    );
    std::fs::write(dir.join("conformance.md"), &page)?;
    println!("wrote site/content/reference/conformance.md");
    Ok(())
}

/// Drop a generated document's own `# Title`, which the page template renders instead.
///
/// Two `<h1>`s on a page is the kind of thing that costs nothing to get right and is
/// quietly wrong for both screen readers and search engines.
fn strip_leading_h1(markdown: &str) -> String {
    let rest = markdown.trim_start();
    match rest.strip_prefix("# ") {
        Some(after) => after
            .split_once('\n')
            .map_or(String::new(), |(_, body)| body.trim_start().to_string()),
        None => rest.to_string(),
    }
}

fn tick(yes: bool) -> &'static str {
    if yes { "yes" } else { "—" }
}

fn join_kinds(kinds: &[s2_kit::message::MessageKind]) -> String {
    if kinds.is_empty() {
        return "—".to_string();
    }
    kinds
        .iter()
        .map(|k| format!("`{}`", k.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// The standard's own field documentation
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct DocType {
    type_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fields: BTreeMap<String, DocField>,
    #[serde(default)]
    variants: BTreeMap<String, DocVariant>,
}

#[derive(Debug, serde::Deserialize)]
struct DocField {
    field_name: String,
    field_type: String,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    description: String,
}

#[derive(Debug, serde::Deserialize)]
struct DocVariant {
    variant_name: String,
    #[serde(default)]
    description: String,
}

fn load_structured_docs(root: &Path) -> Result<Vec<DocType>> {
    let dir = root.join("specs/s2-documentation/structured-documentation");
    if !dir.exists() {
        bail!(
            "specs/s2-documentation is not present (it is gitignored); clone it to run this task"
        );
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let text = std::fs::read_to_string(&path)?;
            let doc: DocType =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            out.push(doc);
        }
    }
    out.sort_by(|a, b| a.type_name.cmp(&b.type_name));
    Ok(out)
}

/// Writes the data-model reference: one page per S2 type, from the standard's own text.
fn gen_model_reference(root: &Path) -> Result<()> {
    let docs = load_structured_docs(root)?;
    let dir = content(root).join("reference/model");
    std::fs::create_dir_all(&dir)?;

    let mut index = format!(
        "{}{}",
        front_matter(
            "Data model",
            "Every type of the S2 data model — all 36 messages and their components — \
             with the field-by-field descriptions from the standard's own documentation.",
            Some(30),
        ),
        "Generated from the S2 documentation project's `structured-documentation`, which\n         that project calls \"the single source of truth for other places where the data\n         model is documented\".\n\n         Where this crate's own API documentation differs from the text below, the\n         difference is deliberate and carries an `E` number in the\n         [errata](@/docs/errata.md).\n\n         | Type | Fields |\n|---|---|\n",
    );

    for doc in &docs {
        let summary = if doc.description.trim().is_empty() {
            format!(
                "The `{}` type of the S2 energy flexibility standard: its fields, their \
                 types, and which are required.",
                doc.type_name
            )
        } else {
            // One sentence, trimmed to something a search result can show whole.
            let text = doc.description.trim().replace('\n', " ");
            let first = text
                .split_once(". ")
                .map_or(text.clone(), |(a, _)| format!("{a}."));
            first.chars().take(155).collect()
        };
        let mut page = front_matter(&doc.type_name, &summary, None);
        if !doc.description.trim().is_empty() {
            page.push_str(doc.description.trim());
            page.push_str("\n\n");
        }
        if !doc.fields.is_empty() {
            page.push_str("| Field | Type | Required | Description |\n|---|---|---|---|\n");
            let mut fields: Vec<_> = doc.fields.values().collect();
            fields.sort_by(|a, b| a.field_name.cmp(&b.field_name));
            for f in fields {
                writeln!(
                    page,
                    "| `{}` | `{}` | {} | {} |",
                    f.field_name,
                    f.field_type,
                    if f.optional { "no" } else { "**yes**" },
                    f.description.trim().replace('\n', " ")
                )?;
            }
        }
        if !doc.variants.is_empty() {
            page.push_str("\n| Value | Description |\n|---|---|\n");
            let mut variants: Vec<_> = doc.variants.values().collect();
            variants.sort_by(|a, b| a.variant_name.cmp(&b.variant_name));
            for v in variants {
                writeln!(
                    page,
                    "| `{}` | {} |",
                    v.variant_name,
                    v.description.trim().replace('\n', " ")
                )?;
            }
        }
        std::fs::write(dir.join(format!("{}.md", doc.type_name)), page)?;
        writeln!(
            index,
            "| [`{}`](@/reference/model/{}.md) | {} |",
            doc.type_name,
            doc.type_name,
            doc.fields.len().max(doc.variants.len())
        )?;
    }
    std::fs::write(dir.join("_index.md"), index)?;
    println!("wrote {} pages to site/content/reference/model", docs.len());
    Ok(())
}

/// Checks the Rust model against the standard's own field documentation.
///
/// The point is that a field can exist in the schema, be documented by the standard, and
/// simply be missing from the Rust struct — and nothing else in the build would notice,
/// because a struct that does not mention a field still round-trips every test fixture
/// that also does not mention it.
fn check_docs(root: &Path) -> Result<()> {
    let docs = load_structured_docs(root)?;
    let rust = rust_model(root)?;

    let mut problems = Vec::new();
    let mut checked = 0usize;
    for doc in &docs {
        // The Rust name drops the control-type prefix: `FRBC.OperationMode` lives at
        // `frbc::OperationMode`.
        let short = doc
            .type_name
            .split_once('.')
            .map_or(doc.type_name.as_str(), |(_, rest)| rest);
        let Some(fields) = rust.get(short) else {
            if !doc.fields.is_empty() {
                problems.push(format!(
                    "{}: documented by the standard, absent from the Rust model",
                    doc.type_name
                ));
            }
            continue;
        };
        for f in doc.fields.values() {
            // `message_type` is the serde tag on the `Message` enum, not a field on any
            // payload struct: it is written and read exactly once, by the discriminant,
            // which is why no message struct carries it.
            if f.field_name == "message_type" {
                continue;
            }
            checked += 1;
            if !fields.contains(&f.field_name) {
                problems.push(format!(
                    "{}.{}: in the standard's documentation, not in the Rust struct",
                    doc.type_name, f.field_name
                ));
            }
        }
    }

    if problems.is_empty() {
        println!("check-docs: {checked} documented fields all present in the Rust model");
        Ok(())
    } else {
        for p in &problems {
            eprintln!("  {p}");
        }
        bail!("{} problems", problems.len())
    }
}

/// A deliberately small reader of `src/types/*.rs`: struct name to the set of wire field
/// names, honouring `#[serde(rename = "...")]`.
fn rust_model(root: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let dir = root.join("src/types");
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path)?;
            let mut current: Option<String> = None;
            let mut rename: Option<String> = None;
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("pub struct ") {
                    let name = rest
                        .split(|c: char| c == ' ' || c == '{' || c == '<' || c == ';')
                        .next()
                        .unwrap_or_default();
                    current = Some(name.to_string());
                    continue;
                }
                if trimmed == "}" {
                    current = None;
                    rename = None;
                    continue;
                }
                if let Some(name) = current.as_ref() {
                    // `rename` may sit on its own line inside a multi-line `#[serde(...)]`
                    // attribute, which is how every field with a documentation comment
                    // and a rename is actually formatted.
                    if trimmed.contains("rename = \"") {
                        rename = trimmed
                            .split("rename = \"")
                            .nth(1)
                            .and_then(|s| s.split('"').next())
                            .map(str::to_string);
                        continue;
                    }
                    if trimmed.starts_with('#') || trimmed.starts_with(")]") {
                        continue;
                    }
                    if let Some(rest) = trimmed.strip_prefix("pub ") {
                        if let Some((field, _)) = rest.split_once(':') {
                            let wire = rename.take().unwrap_or_else(|| field.trim().to_string());
                            out.entry(name.clone()).or_default().insert(wire);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}
