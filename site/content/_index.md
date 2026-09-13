+++
title = "s2-kit"
description = "The S2 energy flexibility standard (EN 50491-12-2) in Rust: a data model proven against the official JSON schemas, a 65-rule semantic validator, sans-I/O session engines for both CEM and RM, and S2 Connect from discovery to an authorised WebSocket."
template = "index.html"
+++

## What S2 is

**S2** — formally **EN 50491-12-2**, becoming IEC 63402-2 — is the European standard for
communicating *energy flexibility* between a **Customer Energy Manager** (CEM) and a
**Resource Manager** (RM). A battery, an EV charger, a heat pump or a PV inverter
describes **how** it can behave; the energy manager decides **why** it should.

The standard's own image is a menu: the resource writes it, the manager orders from it,
and the resource may always refuse. That last part matters — S2 is not a remote control.
A device never surrenders authority over itself, which is what makes it safe to let a
grid operator influence a freezer.

<div class="grid">
<div>

### Five control types

Devices differ in *what kind of promise* they can make, so S2 defines five shapes of
flexibility: **PEBC** (stay inside a power envelope), **PPBC** (run this sequence, when
you like), **OMBC** (pick one of these operation modes), **FRBC** (fill a buffer at a
rate), **DDBC** (meet a demand). A heat pump with a hot-water tank is FRBC; a washing
machine is PPBC.

</div>
<div>

### Two specifications

**S2 JSON** is the data model and the protocol around it — 36 messages, control-type
selection, reception statuses, revocation, validity windows, timers, instruction
lifecycles. **S2 Connect** is how two devices find each other, how a person's trust in
both becomes a shared secret, and how that secret becomes a WebSocket.

</div>
<div>

### Why a library

Every implementation so far stops at *parse, acknowledge, dispatch*. The state table,
revocation, validity windows, timers and instruction resolution get rebuilt in each
application — and each rebuild gets a different subset right.

</div>
</div>

## What s2-kit gives you

<div class="grid">
<div>

### A model that is provably the wire

The Rust types are hand-written and **proven against the official JSON schemas in CI, in
both directions**: every message is validated by a real JSON Schema validator, and every
property the schema defines must exist in the Rust type. All 273 fields the standard
documents are present, checked by a tool that reads the standard's own documentation.

</div>
<div>

### A semantic validator

65 numbered rules, each quoting the sentence of the standard it implements, each with a
test that fires it. The rule identifier travels in the `diagnostic_label` of every
failing `ReceptionStatus`, so a refusal is greppable across a fleet rather than a
mystery — and `s2-kit replay` runs the whole catalogue over a recorded conversation.

</div>
<div>

### Session engines for both roles

Sans-I/O: no sockets, no clock, no tasks. Every engine takes `now` as a parameter, so a
five-second acknowledgement timeout, a description that becomes valid at midnight and a
timer that blocks a transition are ordinary unit tests that run in microseconds.

</div>
<div>

### S2 Connect, end to end

Discovery over DNS-SD, the mutual HMAC challenge–response with its certificate binding,
the per-node rate limit, the two-phase token commit, and an `axum` endpoint — through to
the WebSocket the communication token opens, over a real TLS connection.

</div>
<div>

### Tested against implementations it did not write

The official crate is linked and every message round-tripped through its model; the
official example Resource Managers are built and talked to over a real socket. Five
defects in them are caught by it — including that the official crate cannot read 25 of
the 43 messages in the standard's own published walkthroughs.

</div>
</div>

## Ten lines

```rust
use s2_kit::prelude::*;

let details = ResourceManagerDetails::builder()
    .resource_id(Id::new_const("battery-1"))
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
```

## Where to go next

- **[Get started](@/docs/getting-started.md)** — install it and run a conversation with no
  network at all.
- **[Guide](@/docs/_index.md)** — validation, sessions, and S2 Connect explained.
- **[Rule catalogue](@/reference/rules.md)** — all 65 rules and their sources.
- **[Conformance](@/reference/conformance.md)** — what is implemented, and what is not
  claimed.
- **[Errata](@/docs/errata.md)** — the rough edges found in the specification, and how
  they are handled.
