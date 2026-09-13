# s2-kit

[![crates.io](https://img.shields.io/crates/v/s2-kit)](https://crates.io/crates/s2-kit)
[![docs.rs](https://img.shields.io/docsrs/s2-kit)](https://docs.rs/s2-kit)
[![license](https://img.shields.io/crates/l/s2-kit)](#licence)

**⚡ The S2 energy flexibility standard in Rust.**

S2 — formally **EN 50491-12-2**, becoming IEC 63402-2 — is the European standard for
communicating energy flexibility between a **Customer Energy Manager** (CEM) and a
**Resource Manager** (RM). A battery, an EV charger, a heat pump or a PV inverter
describes *how* it can behave; the energy manager decides *why* it should. The standard's
own image is a menu: the resource writes it, the manager orders from it, and the resource
may always refuse.

`s2-kit` implements the two open specifications that make the standard usable over IP:

* **S2 JSON v1.0.0** — the 36 messages and 41 component types, their encoding, and the
  protocol around them: control-type selection, reception statuses, revocation, validity
  windows, timers and instruction lifecycles.
* **S2 Connect 1.0.0** — discovery, the pairing and session-initiation state machines, the
  HMAC challenge–response with its certificate binding, token rotation, and both HTTPS
  drivers.

```console
$ cargo add s2-kit
```

📖 **[Documentation and guides](https://hupe1980.github.io/s2-kit)** ·
[API reference](https://docs.rs/s2-kit) ·
[Rule catalogue](https://hupe1980.github.io/s2-kit/reference/rules/) ·
[Conformance](https://hupe1980.github.io/s2-kit/reference/conformance/) ·
[Changelog](CHANGELOG.md)

---

## 🤔 Why another one

There is an official Rust crate, [`s2energy`](https://crates.io/crates/s2energy), and this
one exists because three things are missing from the ecosystem as a whole.

**📐 A model that is provably the wire.** `s2energy` generates its types with `typify` from a
*modified* copy of the schema, and the modification matters: it declares `ID` as
`format: uuid` although both tagged versions of S2 JSON define it as the pattern
`[a-zA-Z0-9\-_:]{2,64}`. The result rejects every identifier in the standard's own worked
examples — `"actuator1"`, `"om1"` — and every conforming peer that does not use UUIDs.
`s2-kit`'s types are hand-written and **proven** against the official schemas in CI, in
both directions: every message is validated by a real JSON Schema validator, and every
property the schema defines must exist in the Rust type.

**⚙️ A protocol engine.** Every existing implementation stops at "parse, acknowledge,
dispatch". The state table, revocation, validity windows, timers and instruction
lifecycles are left to each application to rebuild. `s2-kit` ships them, for both roles,
sans-I/O.

**📋 Conformance.** The official certification-tool repository contains a licence file and
nothing else; the S2 Analyzer validates only against the schema. `s2-kit` has a
[rule catalogue](https://hupe1980.github.io/s2-kit/reference/rules/) of 61 numbered
semantic rules, each quoting the sentence it implements and each with a test that fires
it — and an `Analyzer` that runs all of them over a whole recorded conversation.

## ⚡ Ten lines

```rust
use s2_kit::prelude::*;

let details = ResourceManagerDetails::builder()
    .resource_id(Id::parse("battery-1")?)
    .roles(vec![Role::new(RoleType::EnergyStorage, Commodity::Electricity)])
    .instruction_processing_delay(Duration::from_millis(500))
    .available_control_types(vec![ControlType::FillRateBasedControl])
    .provides_forecast(false)
    .provides_power_measurement_types(vec![CommodityQuantity::ElectricPower3PhaseSymmetric])
    .build();

let mut rm = RmSession::new(RmConfig::default(), details);
rm.open(Timestamp::now());

// Everything the session wants to say is waiting here; a driver writes it to a socket.
let hello = rm.poll_transmit().expect("the Resource Manager speaks first");
assert_eq!(hello.kind, MessageKind::Handshake);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Run a complete conversation with no network at all:

```console
$ cargo run --features testing --example battery_rm
$ cargo run --example connect_pair --features connect-client,connect-server
```

## 📦 What you get

**🔒 Acknowledgements that cannot deadlock.** S2 requires a `ReceptionStatus` for every
message except a `ReceptionStatus`, and getting it wrong is the ecosystem's most common
bug — [`s2-json#22`][22] is open about the deadlocks that follow when an implementation
*awaits* the acknowledgement it owes. `s2-kit` chooses the status inside `handle_text`,
synchronously, and tracks every outstanding one in a ledger with a deadline. A peer that
never answers produces an event, not a stall.

**🎯 Instructions that arrive resolved.** An S2 instruction names an operation mode by
identifier and gives a factor between zero and one; turning that into watts needs the
description that was sent earlier. `s2-kit` does it once, and says when an instruction
*cannot* be carried out:

```text
FRBC.Instruction instr0 on actuator1 → Charging (charge) at factor 0.6,
  ElectricPower3PhaseSymmetric 3000 W, filling at 0.0015/s
  ; blocked by Minimum discharge time (cooldown)
```

**🔍 Rule identifiers on the wire.** Every failing `ReceptionStatus` carries the rule that
failed, so a refusal is greppable across a fleet rather than a mystery:

```json
{"message_type":"ReceptionStatus","subject_message_id":"mx","status":"INVALID_CONTENT",
 "diagnostic_label":"S2-FRBC-003 at /actuator_id: nope is not an actuator of this system"}
```

**⏱️ A whole fleet on one timer.** Both roles satisfy one `Session` trait, and a `SessionSet`
answers the only question an event loop has: when must I next wake, and for whom. Each
session keeps the counters an operator asks for too — refusals, nacks, ack timeouts, and
the *slowest* acknowledgement, not just the mean.

**🤝 S2 Connect end to end.** Discovery over DNS-SD, the mutual challenge–response bound to
the server's certificate, the per-node rate limit and mandatory delay *in the state
machine* rather than in a request handler, the two-phase token commit with both its expiry
windows, long-polling for the Resource Manager nobody can dial, and an `axum` endpoint that
ends where the specification does: a WebSocket the communication token opens, once. All of
it tested over a real TLS connection on loopback.

**🔐 Your application's TLS backend, not ours.** `s2-kit` never installs a `rustls` crypto
provider over one the application already chose. If you have not chosen, it bundles one:
`ring` by default, because a gateway gets cross-compiled and `ring` needs no `cmake`;
`aws-lc-rs` behind a feature, for FIPS and post-quantum. Exactly one is linked, and the
end-to-end pairing suite runs on both.

## 🏗️ Design

**🧩 Sans-I/O.** No sockets, no clock, no tasks in the protocol core. Every engine takes
`now` as a parameter, so a five-second acknowledgement timeout, a description that becomes
valid at midnight and a timer that blocks a transition are ordinary unit tests that run in
microseconds. Drivers are Cargo features that hang off the side.

```text
types → codec → validate → model → session
                                      └──▶ io                (sockets, clock)
connect::proto ──────────────────────────▶ connect::{client, server, discovery}
```

**🔬 `no_std + alloc`.** The core — cryptography included — builds for
`wasm32-unknown-unknown` and `thumbv7em-none-eabihf`, and 178 of its unit tests run there.

**⚖️ Representable is not valid.** A factor of 1.3 and a 289-element forecast are both
representable, because a proxy has to carry a message it would refuse to send. The types
accept what the schema accepts; the validator judges; the builders refuse.

**🧱 No third-party type in the public API.** Timestamps are `s2_kit::Timestamp`, with
conversions to `jiff`, `time` and `chrono` behind features, and each crate re-exported so
a caller can always name the version this crate was built against.

**🔀 Both wire profiles.** The deployed ecosystem negotiates `"0.0.2-beta"` while the schemas
are tagged `v1.0.0`. They differ in exactly one place — DDBC's present demand rate moved
from a field to a message — so `s2-kit` speaks both and enforces whichever was negotiated.
Negotiation is an exact string match, never a semver range — with one deliberate latitude:
the two specifications spell the *same* version differently (S2 Connect requires `v1.0.0`,
every S2 JSON handshake writes `1.0.0`, and the one other Rust implementation's examples
write `v1`), so matching ignores a leading `v` and nothing else. Three spellings compared
with `==` is a pairing that fails between two implementations that speak the same version.

## 🎛️ Features

| Feature | Enables |
|---|---|
| `std` *(default)* | The standard library. Without it the crate is `no_std + alloc`. |
| `uuid` *(default)* | `Id::generate()` and friends. |
| `tokio` | `io::Driver`, which pumps a session over any text transport, and a WebSocket. |
| `connect-client` | Pairing and session initiation over `reqwest`/`rustls`, with leaf-fingerprint capture and CA pinning. |
| `connect-server` | An `axum` router for both APIs and the authorised WebSocket, TLS serving, and a self-signed CA helper. |
| `discovery` | DNS-SD advertise and browse of `_s2connect._tcp`, pure Rust (no Avahi). |
| `tls-ring` *(default)* | 🔐 `ring` as the TLS backend: pre-generated assembly, builds with `cc`, no `cmake`. |
| `tls-aws-lc-rs` | 🔐 `aws-lc-rs` instead: rustls's own default, FIPS-certifiable, post-quantum key exchange — at the cost of a vendored BoringSSL and a `cmake` build. |
| `jiff` / `time` / `chrono` | `Timestamp` conversions, with the crate re-exported. |
| `testing` | Fixtures, the two-engine harness, and `.s2log` transcripts read as well as written. |
| `tracing` | Structured events from the drivers: what crossed the wire, and what was refused. |
| `schemars` | `JsonSchema` for this crate's own types. |
| `cli` | The `s2-kit` command-line tool. |

## 🖥️ Command-line tool

```console
$ cargo install s2-kit --features cli

$ s2-kit validate message.json
message 1: S2-NUM-004 at /operation_mode_factor: operation mode factor 1.3 is outside [0, 1]
1 message(s) checked against 61 rules: FAILED

$ s2-kit replay session.s2log
   4 cem>rm FRBC.Instruction: S2-FRBC-003 at /actuator_id: nope is not an actuator of this system
   7 rm>cem FRBC.StorageStatus: never answered; no ReceptionStatus names m7
12 line(s), 9 answered, checked against 61 rules: FAILED
```

`validate` judges each message alone. `replay` drives a recorded conversation through
`session::Analyzer` — one registry fed from **both** directions — so the rules that need
history fire too, and it reports where the peer's answer differs from the one this crate
would have sent, and which messages nobody answered at all.

Useful in *other* implementations' CI: the validator has no opinion about what wrote the
message.

## ✅ Status

**Not yet published.** Both specifications are implemented end to end — S2 JSON with its
two session engines, and S2 Connect from DNS-SD through pairing to an authorised
WebSocket. What remains is interoperability runs against other implementations.

Verified on every commit: **360 tests** (193 of them under `no_std`), zero clippy warnings
at `pedantic` on every configuration built, the feature powerset to depth 2, `wasm32`,
`thumbv7em` and `riscv32imac` builds, rustdoc with warnings denied, `cargo deny`, and
seven fuzz targets nightly.

* 📐 **The model is the schema.** One instance of every message, with every optional field
  set, validated by a real JSON Schema validator against the vendored official files — and
  every property the schema defines must exist in the Rust type.
* 📖 **The standard's own examples.** All 43 JSON messages from the EV, heat-pump, PV and
  no-control walkthroughs decode, re-encode, validate without a single error, and are fed
  to a session rather than merely parsed.
* 💬 **The documented conversations**, run end to end between two real engines on a virtual
  clock, including the failure modes: a lost acknowledgement, a duplicate delivery, an
  out-of-order message, a permanent error, a blocked timer, a scheduled description.
* 🔌 **S2 Connect over a real socket**, with a real handshake, a real mandatory second and
  real status codes — through to the last mile: the communication token opens the
  WebSocket, carries a session between two engines, and is refused the second time.
* 📋 **Rule coverage.** The suite enumerates the catalogue and requires a case for each rule,
  so a rule cannot be documented without being enforced.
* 🎲 **Properties, not just examples.** Decoding, validating and feeding a session arbitrary
  bytes is total; timestamps, durations, identifiers and messages round-trip; `factor_of`
  undoes `at_factor`. This is what caught that interpolating to a factor of exactly 1 did
  not return the range's own endpoint — the number both roles compare against.
* 🧪 **Fuzzing where it matters most.** Seven targets, including the S2 Connect request
  bodies, which an *unauthenticated* peer reaches before any secret has been checked.
* 🧱 **Bounded where it counts.** Everything an unauthenticated caller picks the size of has
  a named cap — the body, the pairing token, the node alias, the lenient decoder's
  recursion, and the long-polling table — and the last two are proved across a real HTTP
  boundary.

## 🐛 Errata

Implementing a standard carefully means finding its rough edges. Twenty-six are recorded
with how each is handled — among them an `ID` documented as a UUID and defined as a
pattern that is not one, two control-type descriptions that are swapped, `NOT_CONTROLABLE`
and `supported_commodites` misspelled on the wire, a pairing rate limit that bounds online
guessing while the same endpoint hands out an offline oracle, and two specifications that
spell one version number two different ways and then compare it with `==`.

Two of them are in the standard's own published examples. The EV walkthrough's
`FRBC.ActuatorStatus` names operation mode `"string"` while its own description declares
`om1` and `om2`; the heat-pump walkthrough declares `actuator1` and then instructs
`actuator0`. Every value matches the `ID` pattern, so a schema validator sees nothing, and
every message is faultless alone, so a message-at-a-time validator sees nothing either.
The fixtures stay verbatim: a crate that corrects the standard's examples to make its own
tests pass has stopped testing against them.

See the **[errata](https://hupe1980.github.io/s2-kit/docs/errata/)**. A validator that
refuses the standard's own published examples is a bug, not a feature.

## 🔗 Related

* [S2 standard documentation](https://docs.s2standard.org/) · [S2 JSON](https://github.com/flexiblepower/s2-json) · [S2 Connect](https://github.com/flexiblepower/s2-connect)
* [`s2energy`](https://crates.io/crates/s2energy) — the official Rust crate
* [`s2-python`](https://pypi.org/project/s2-python/) · [`s2-ruby`](https://github.com/stekker/s2-ruby)
* [S2 Discord](https://discord.com/invite/NyFMEPmuDw)

## ⚖️ Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

[22]: https://github.com/flexiblepower/s2-json/issues/22
