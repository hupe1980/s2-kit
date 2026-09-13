# Changelog

Notable changes to `s2-kit`, in the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
format. Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Before 1.0 a minor bump may break the API. What will *not* break silently: rule
identifiers (`S2-FRBC-004`) and the `D`, `R` and `E` numbers cited from the code keep
their meaning, and a change to what a rule means gets a new identifier rather than a new
definition.

## [Unreleased]

Nothing yet.

## [0.1.0] — unreleased

First release. Implements **S2 JSON v1.0.0** and **S2 Connect 1.0.0**.

### Added

#### Data model and codec

- All 36 messages and 41 component types, hand-written and checked against the official
  JSON schemas in both directions in CI.
- `Id` as an inline `Copy` string matching the schema's pattern, not a UUID; `Timestamp`
  and `Duration` with no time-crate dependency, and conversions to `jiff`, `time` and
  `chrono` behind features.
- Two-phase decode, so every failure can still be answered with the right
  `ReceptionStatus`; canonical byte-stable encoding; a 1 MiB cap applied before parsing.
- Both wire profiles (`1.0.0` and `0.0.2-beta`), enforced per negotiated version.

#### Validator

- 61 numbered semantic rules — 42 errors, 19 warnings — each quoting the sentence it
  implements and each with a test that fires it. The identifier travels in the
  `diagnostic_label` of every failing `ReceptionStatus`.
- `session::Analyzer`: one registry fed from **both** directions, so the cross-message
  rules run on a conversation nobody is a party to.

#### Session engines

- `RmSession` and `CemSession` over one sans-I/O `SessionCore`: the state table as data,
  an acknowledgement ledger that cannot deadlock, registries with validity windows,
  timers, revocation and instruction lifecycles.
- `Instructed` events carrying a resolved `model::Explanation` — actuator, operation mode,
  factor, the power it implies, and the timers in the way.
- `SessionSet` for many resources on one timer, and `Stats` counters per session.
- `.s2log` transcripts written and read, and `testing::replay`, which reports the rules a
  recorded conversation broke, the messages nobody answered, and the answers that differ
  from the ones this crate would have sent.

#### S2 Connect

- Pairing and session-initiation state machines for both sides, `no_std`, with the
  mandatory one-second delay and the per-node rate limit in the state machine.
- Access tokens with both expiry windows, and communication tokens the server keeps and
  spends — valid for one connection and thirty seconds.
- `connect::tls`: leaf-fingerprint capture from a live handshake, three trust policies,
  and a self-signed endpoint helper.
- `connect-client` over `reqwest`/`rustls`, including long-polling and a WebSocket dialler
  that verifies against the CA pairing pinned.
- `connect-server` over `axum`: pairing, session initiation, the LAN-only operations
  behind a same-subnet check, the long-polling queue, and an authorised WebSocket route.
- DNS-SD advertise and browse of `_s2connect._tcp`, pure Rust.

#### Drivers and tooling

- `io::Driver` over any `TextTransport`, with keep-alive pings measured from the last ping.
- `s2-kit validate`, `s2-kit replay` and `s2-kit rules` behind the `cli` feature.
- A documentation site with the generated rule catalogue, conformance statement and data
  model reference.

### Known issues

- Interoperability has been proved only against this crate itself. Running against
  `s2-python`, `s2energy-connection` and the reference implementations is outstanding.
- Twenty-six specification defects are recorded in the
  [errata](https://hupe1980.github.io/s2-kit/docs/errata/), two of them in the standard's
  own published examples. The fixtures are kept verbatim and the findings asserted by
  name.

[Unreleased]: https://github.com/hupe1980/s2-kit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hupe1980/s2-kit/releases/tag/v0.1.0
