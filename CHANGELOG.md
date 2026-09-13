# Changelog

Notable changes to `s2-kit`, in the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
format. Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Before 1.0 a minor bump may break the API. What will *not* break silently: rule
identifiers (`S2-FRBC-004`) and the `D`, `R`, `E` and `I` numbers cited from the code keep
their meaning, and a change to what a rule means gets a new identifier rather than a new
definition.

## [Unreleased]

Nothing yet.

## [0.3.0] — unreleased

Breaking. A crypto-provider fix that changes what the `tokio` feature pulls in, and three
API corrections that came out of `hems`'s report on 0.2.0.

### Breaking

- `S2-RMD-003` is keyed on the `(role, commodity)` **pair**, not the commodity alone:
  `roles` is capped at three, there are three `RoleType`s and four `Commodity`s, so the
  cap counts role types and a battery may be storage, consumer *and* producer of
  electricity (E30). The identifier is kept — the condition is narrowed, not replaced.
- `CemEvent::InstructionStatus` carries the whole `InstructionStatusUpdate`. The old
  projection dropped `timestamp`, which is when the status last *changed* rather than when
  the message arrived (D53).
- `ProtocolVersion::V1_0_0` and `V0_0_2_BETA` are associated constants of the type, not
  `&'static str`, joined by `V1_0_0_CONNECT`/`V0_0_2_BETA_CONNECT` for the `v` spelling
  S2 Connect requires (E25). The type wraps a `Cow<'static, str>`, so naming a version
  allocates nothing: `ProtocolVersion::new(ProtocolVersion::V1_0_0)` is now `::V1_0_0`.
- The `tokio` feature activates `rustls`, `rustls-pki-types` and
  `rustls-platform-verifier`. `connect::tls`, `TlsPolicy` and `WebSocketOptions::policy`
  are available whenever `tokio` is, having been `connect-client`-only.

### Added

- `io::Dialled`, the name for `WebSocket<MaybeTlsStream<TcpStream>>`.
- Crate-root re-exports of every dependency whose types reach the public API: `tokio`,
  `tokio_tungstenite`, `rustls` (with `tokio`), `axum` (`connect-server`), `mdns_sd`
  (`discovery`), joining `chrono`/`jiff`/`time`/`uuid`.
- `tests/manifest.rs`: invariants of the feature graph, which neither `cargo tree` nor the
  compiler can see.

### Fixed

- **`--features tokio` compiled a TLS stack with no crypto provider, and the first
  `wss://` dial panicked inside `rustls`**: `rustls` arrived transitively through
  `tokio-tungstenite`, so `tls-ring = ["rustls?/ring", …]` reached nothing. Every dial now
  supplies its own `Connector::Rustls` from `connect::tls::default_provider()`, which also
  makes the ambiguous case — an application that installed `aws-lc-rs` against a
  `tls-ring` build — unreachable, and a provider-less build an `Error` (D52).
- `tokio-tungstenite`'s `rustls-tls-webpki-roots` is dropped, removing a second copy of
  Mozilla's root list that nothing read. `WebSocket::connect` verifies against the
  platform trust store, as `TlsPolicy::Web` always did.
- `tests/model_matches_schema.rs` and the generated site doctests used `s2_kit::testing`
  without requiring the feature, so `cargo test` and `cargo test --doc` failed to compile
  on the default feature set.
- `cargo xtask interop` built the peers from `target/interop/<peer>`, keyed on the
  directory existing. CI caches `target/` and restores that directory without the sources,
  so cargo walked up, built s2-kit instead and reported success with no peer binary. The
  manifest is the marker now, and a build that produces no binary re-copies and retries.

## [0.2.0] — 2026-09-13

Breaking. Nothing else implements S2 JSON v1.0.0, so most of what changed came from
running against the implementations that do exist — the official crate, linked; and the
official example Resource Managers, as processes.

### Breaking

- The state table is keyed by `(Phase, Role, MessageKind)`. `Phase::Negotiating` is a row
  the standard's own table does not have: until a bare-WebSocket session has finished its
  `Handshake`, nothing but the handshake crosses. `validate::allowed` and
  `Context::phase` take it; `Context::active_control_type` is now a method.
- `SessionSet::drain_transmit` and `drain_events` return owned keys, so a drain can be
  acted on.
- `CemEvent::TimerReported` replaces `TimerFinished`: a past `finished_at` means the timer
  ran out *or* never started, and the old name asserted the first.
- `Explanation` carries `unresolved: Option<ResolveError>`, and `is_actionable()` is false
  whenever it is set.
- `model::schedule` requires one choice per container in the profile's own order;
  `ScheduleProblem::NotSchedulable` replaces a misreported `WrongCount`.
- `prune_unknown` and `schema::type_spec_in` take a `WireProfile`.
- `io::WebSocket::open` with `WebSocketOptions`; `TextTransport::recv_text` is documented
  as requiring cancel-safety.
- Rules: `S2-MSG-008` (profile field), `S2-STATUS-002` (unknown timer), `S2-PPBC-007`,
  `S2-PPBC-008`. `S2-PPBC-005` no longer refuses a `progress` the standard permits.

### Added

- **Interoperability.** `tests/interop_s2energy.rs` links the official crate and
  round-trips the 35 messages of `0.0.2-beta` through its model; `cargo xtask interop`
  builds the official example Resource Managers and talks to them over a real socket with
  every row's outcome pinned. Together they found five defects in those implementations.
- `testing::every_message()`, `schema::Kind::Id`, `RmConfig::after_failed_attempts`,
  `CemEvent::OutboundWarnings`, `Endpoint::with_max_message_bytes`,
  `client::MAX_RESPONSE_BYTES`, `CommodityQuantity::unit`/`as_str`.

### Fixed

- Lenient decoding pruned a field `0.0.2-beta` requires and then refused the message for
  missing it, so an `Analyzer` on that profile could not read a `DDBC.SystemDescription`.
- An outbound message could be sent before the session was open, and an invalid
  profile-specific field was never caught on the way out.
- Frames were bounded at the parser but not at the WebSocket handshake, and client
  response bodies were not bounded at all.
- A failed pairing left the server's attempt in flight for the whole fifteen-second
  budget, refusing every retry with `503`.
- A dual-role DNS-SD endpoint silently advertised only one of its two required subtypes.
- `PPBC_WINDOW_TOO_SHORT` checked each container against the whole window rather than the
  containers against each other.
- `NodeIdAlias` could not be deserialised from anything that had already been parsed.
- `replay` was quadratic.

## [0.1.0] — 2026-09-13

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
- Both wire profiles (`1.0.0` and `0.0.2-beta`), enforced per negotiated version in **both
  directions** — the codec refuses the wrong shape on the way in, rule `S2-MSG-008` on the
  way out — and the lenient decoder prunes against the negotiated profile's own property
  table, so a conforming `0.0.2-beta` message survives being read by a proxy.

#### Validator

- 65 numbered semantic rules — 45 errors, 20 warnings — each quoting the sentence it
  implements and each with a test that fires it. The identifier travels in the
  `diagnostic_label` of every failing `ReceptionStatus`.
- `session::Analyzer`: one registry fed from **both** directions, so the cross-message
  rules run on a conversation nobody is a party to.

#### Session engines

- `RmSession` and `CemSession` over one sans-I/O `SessionCore`: the state table as data —
  keyed by `(Phase, Role, MessageKind)`, with a `Phase::Negotiating` row the standard's own
  table does not have, so nothing but the handshake crosses a bare-WebSocket session before
  the version is agreed — an acknowledgement ledger that cannot deadlock, registries with
  validity windows, timers, revocation and instruction lifecycles.
- `Instructed` events carrying a resolved `model::Explanation` — actuator, operation mode,
  factor, the power it implies, and the timers in the way.
- `SessionSet` for many resources on one timer, and `Stats` counters per session.
- `.s2log` transcripts written and read, and `testing::replay`, which reports the rules a
  recorded conversation broke, the messages nobody answered, and the answers that differ
  from the ones this crate would have sent.

#### S2 Connect

- Pairing and session-initiation state machines for both sides, `no_std`, with the
  mandatory one-second delay and the per-node rate limit in the state machine.
- A client that gives up **releases** the pairing attempt it started, instead of leaving
  the device refusing every retry with `503` for the rest of the fifteen-second budget.
  The `pairingAttemptId` is remembered before the server's answer is judged, which is what
  makes the release reachable from a failure at all.
- Bounded response reads (`client::MAX_RESPONSE_BYTES`): during a LAN pairing the server is
  deliberately unauthenticated, so what it answers with has to be bounded too.
- `confirmAccessToken` is sent with no request body, which is what the OpenAPI defines.
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
  `TextTransport::recv_text` is documented as requiring cancel-safety, because the driver
  races it against a timer on every step.
- The codec's size cap applied at the WebSocket handshake as well as at the parser
  (`io::WebSocketOptions`, `Endpoint::with_max_message_bytes`), so a default build is
  bounded where memory is actually held rather than only where it is parsed.
- `s2-kit validate`, `s2-kit replay` and `s2-kit rules` behind the `cli` feature, both
  verbs taking `--beta` to read a stream as S2 JSON `0.0.2-beta`.
- A documentation site with the generated rule catalogue, conformance statement and data
  model reference.

#### Interoperability

- `cargo xtask interop` — this crate's CEM against the official FlexiblePower example
  Resource Managers, built from source and talked to over a real WebSocket, with each
  row's expected outcome pinned so the matrix is a gate rather than a report. It found
  three defects in those peers.
- `examples/interop_cem.rs`, the bare-WebSocket CEM server the peers dial.
- `tests/interop_s2energy.rs` — the official `s2energy` crate as a **dev-dependency**, with
  the 35 messages of `0.0.2-beta` round-tripped through its model — the only profile any
  other implementation speaks. No container and no subprocess. It
  showed that the official crate refuses 25 of the 43 messages in the standard's own
  walkthroughs (erratum E1, as a number) and that it speaks only `0.0.2-beta`.
- `testing::every_message()` — one of every message with every optional field set, shared
  by the schema-equivalence and interoperability suites.
- `schema::Kind::Id` / `Kind::IdArray`, generated: which properties hold S2 identifiers, so
  a tool that walks a message can follow them without a hand-maintained field list.

### Known issues

- Interoperability reaches the two Rust example implementations and no further: the peers
  that need a container runtime (`s2-python`, `cem-reference-1`, `s2-analyzer` in the
  middle) are outstanding, the matrix runs one role and one wire profile, and its peers
  speak bare WebSocket — so nothing in it exercises S2 Connect against another
  implementation.
- There is no official conformance suite to run against: `flexiblepower/s2-certification-tool`
  is one commit and a licence file.
- Twenty-seven specification defects are recorded in the
  [errata](https://hupe1980.github.io/s2-kit/docs/errata/), two of them in the standard's
  own published examples. The fixtures are kept verbatim and the findings asserted by
  name.

[Unreleased]: https://github.com/hupe1980/s2-kit/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/hupe1980/s2-kit/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/hupe1980/s2-kit/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hupe1980/s2-kit/releases/tag/v0.1.0
