//! `s2-kit` — validate S2 messages and transcripts from the command line.
//!
//! Built for other implementations' continuous integration as much as for this one: it
//! reads a file of S2 JSON, or a `.s2log` transcript, and reports every rule that fires
//! with its stable identifier.
//!
//! ```text
//! s2-kit validate message.json   # one message, or an array, or one per line
//! s2-kit replay session.s2log    # a whole conversation, cross-message rules and all
//! s2-kit rules                   # the catalogue, as Markdown
//! ```
//!
//! The two verbs answer different questions. `validate` judges each message on its own,
//! which is all you can do with a file that is not a conversation. `replay` asks three
//! questions a message-at-a-time validator cannot:
//!
//! * is each message legal **given everything before it** — an instruction naming an
//!   actuator nobody described, a message sent in the wrong state, a reused instruction id;
//! * did the peer answer as this crate would, and if not, which rule explains the
//!   difference — the interop report, in one line;
//! * was every message answered **at all**, which `S2J` requires and is the deadlock
//!   `[s2-json #22]` describes.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use s2_kit::codec::{DecodeOptions, decode_with};
use s2_kit::session::Analyzer;
use s2_kit::testing::{read_s2log, replay_with};
use s2_kit::types::WireProfile;
use s2_kit::validate::{Context, Severity, Validate, rules};

#[derive(Parser)]
#[command(
    name = "s2-kit",
    version,
    about = "Validate S2 messages and transcripts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate a JSON message, a file of them, or one per line.
    ///
    /// Each message is judged on its own. For a recorded conversation use `replay`, which
    /// also runs the rules that need to know what came before.
    Validate {
        /// The file to read. `-` reads standard input.
        path: PathBuf,
        /// Strip properties the schema does not define instead of refusing the message.
        #[arg(long)]
        lenient: bool,
        /// Treat warnings as failures.
        #[arg(long)]
        strict_warnings: bool,
    },
    /// Replay a `.s2log` transcript: cross-message rules, the answers, and the gaps.
    Replay {
        /// The transcript to read. `-` reads standard input.
        path: PathBuf,
        #[command(flatten)]
        options: ReplayOptions,
    },
    /// Print the rule catalogue as Markdown.
    Rules,
}

/// How to read a transcript. A struct rather than four `bool` arguments, because
/// `replay(text, true, false, false, true)` is a call nobody can check by reading.
#[derive(Debug, Clone, Copy, clap::Args)]
struct ReplayOptions {
    /// Refuse a message with a property the schema does not define.
    #[arg(long)]
    strict: bool,
    /// Treat warnings as failures.
    #[arg(long)]
    strict_warnings: bool,
    /// Read the transcript as S2 JSON v0.0.2-beta rather than v1.0.0.
    #[arg(long)]
    beta: bool,
    /// The transcript was carried by S2 Connect, where a handshake is redundant.
    #[arg(long)]
    s2_connect: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("s2-kit: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        Ok(std::io::read_to_string(std::io::stdin())?)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    }
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    match cli.command {
        Command::Rules => {
            print!("{}", s2_kit::validate::rules_markdown());
            Ok(true)
        }
        Command::Validate {
            path,
            lenient,
            strict_warnings,
        } => Ok(validate_text(&read(&path)?, lenient, strict_warnings)),
        Command::Replay { path, options } => Ok(replay_text(&read(&path)?, options)),
    }
}

/// Accepts a single message, a JSON array of them, or one message per line.
fn validate_text(text: &str, lenient: bool, strict_warnings: bool) -> bool {
    let mut options = DecodeOptions::default();
    if lenient {
        options = options.lenient();
    }

    let mut ok = true;
    let mut checked = 0usize;
    for (line_number, chunk) in messages_in(text).into_iter().enumerate() {
        checked += 1;
        let label = format!("message {}", line_number + 1);
        match decode_with(&chunk, &options) {
            Err(error) => {
                println!("{label}: {error}");
                ok = false;
            }
            Ok(decoded) => {
                for path in &decoded.pruned {
                    println!("{label}: pruned {path}");
                }
                let report = decoded.message.validate(&Context::empty());
                for violation in report.violations() {
                    println!("{label}: {violation}");
                    if violation.severity == Severity::Error || strict_warnings {
                        ok = false;
                    }
                }
            }
        }
    }
    println!(
        "{checked} message(s) checked against {} rules: {}",
        rules::RULES.len(),
        if ok { "ok" } else { "FAILED" }
    );
    ok
}

/// Replays a recorded conversation on its own clock and prints what it found.
fn replay_text(text: &str, options: ReplayOptions) -> bool {
    let entries = read_s2log(text);
    if entries.is_empty() {
        eprintln!("s2-kit: no transcript lines found; expected JSON Lines of {{t, dir, msg}}");
        return false;
    }

    let mut analyzer = Analyzer::new().profile(if options.beta {
        WireProfile::V0_0_2Beta
    } else {
        WireProfile::V1_0_0
    });
    if options.strict {
        analyzer = analyzer.strict();
    }
    if options.s2_connect {
        analyzer = analyzer.over_s2_connect();
    }

    let report = replay_with(&entries, analyzer);
    for finding in &report.findings {
        println!("{finding}");
    }
    let failed = report
        .findings
        .iter()
        .any(|finding| finding.is_error() || options.strict_warnings);
    println!(
        "{} line(s), {} answered, checked against {} rules: {}",
        report.lines,
        report.answered,
        rules::RULES.len(),
        if failed { "FAILED" } else { "ok" }
    );
    !failed
}

/// Splits the input into messages: a JSON array's elements, one per line, or the whole
/// document.
fn messages_in(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(trimmed) {
        return items.iter().map(ToString::to_string).collect();
    }
    if trimmed.lines().count() > 1 {
        let lines: Vec<String> = trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                Some(
                    value
                        .get("msg")
                        .map_or_else(|| line.to_string(), ToString::to_string),
                )
            })
            .collect();
        if lines.len() == trimmed.lines().filter(|l| !l.trim().is_empty()).count() {
            return lines;
        }
    }
    vec![trimmed.to_string()]
}
