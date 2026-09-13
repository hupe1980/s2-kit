+++
title = "Getting started"
description = "Install s2-kit, build a Resource Manager session, and run a complete S2 conversation between both roles with no network at all."
weight = 10
+++

```console
$ cargo add s2-kit
```

The default features give you the data model, the codec, the validator and both session
engines, with `std` and identifier generation. Everything that touches a socket is behind
a feature flag, so the core stays `no_std + alloc` and builds for
`thumbv7em-none-eabihf`:

```console
$ cargo add s2-kit --features tokio,connect-client   # a Resource Manager that dials out
$ cargo add s2-kit --no-default-features             # the core, on a controller with no OS
```

## A Resource Manager in ten lines

An S2 session begins with the resource describing itself. `ResourceManagerDetails` is that
description: what the device is, which control types it offers, and how long it needs
between receiving an instruction and acting on it.

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

let hello = rm.poll_transmit().expect("the Resource Manager speaks first");
assert_eq!(hello.kind, MessageKind::Handshake);
```

Nothing here touched a network. `poll_transmit` hands you the bytes; writing them to a
socket is a separate concern, and one you can skip entirely while testing.

`Id::new_const` validates the identifier while the compiler is running, so a typo in a
literal is a build error rather than a runtime one. For an identifier that arrives at run
time — from a configuration file, a serial number — use `Id::parse`, which returns a
`Result`, or `Id::generate()` for a fresh UUID.

## The four calls in, three out

Every engine — both roles — is driven the same way:

```text
session.handle_text(&text, now)       session.poll_transmit()
session.handle_message(message, now)  session.poll_event()
session.handle_timeout(now)           session.poll_timeout()
session.transport_closed(now)
```

`now` is always a parameter. That is the whole trick behind the design: a five-second
acknowledgement timeout, a description that becomes valid at midnight and a timer that
blocks a transition for two hours are ordinary unit tests that finish in microseconds,
because the clock is something you pass rather than something you wait for.

`poll_timeout` tells you when the session next needs waking. When it returns `None`, the
session wants nothing — an idle device costs you no timer at all.

## A whole conversation, with no network

The `testing` feature wires two real engines back to back through an in-memory pipe on a
virtual clock. This is how the standard's own documented conversations are tested, and it
is the fastest way to see what S2 actually looks like on the wire:

```rust
use s2_kit::prelude::*;
use s2_kit::testing::Conversation;

let mut c = Conversation::battery();
c.open();

// Every message that crossed, except the acknowledgements, earned exactly one.
let sent = c.transcript().iter().filter(|e| e.kind != MessageKind::ReceptionStatus).count();
let acks = c.transcript().iter().filter(|e| e.kind == MessageKind::ReceptionStatus).count();
assert_eq!(sent, acks);
```

Or run one from the command line:

```console
$ cargo run --features testing --example battery_rm
```

## Talking to a real peer

When you do want a socket, the `tokio` feature adds a driver that pumps any session over
any text transport:

```rust,ignore
let socket = WebSocket::connect("wss://cem.local/s2", Some(token)).await?;
let mut driver = Driver::new(socket);

loop {
    driver.step(&mut rm).await?;
    while let Some(event) = rm.poll_event() {
        match event {
            RmEvent::Ready { control_type } => { /* send a system description */ }
            RmEvent::Instruction(i) => { /* i.explanation says what to do */ }
            RmEvent::Closed { .. } => return Ok(()),
            _ => {}
        }
    }
}
```

The driver is deliberately thin — it flushes, waits for the first of {a frame, the
session's deadline, the keep-alive interval}, and feeds the result back in. Everything
that *decides* anything is in the session.

`WebSocket::connect` returns an [`io::Dialled`](https://docs.rs/s2-kit/latest/s2_kit/io/type.Dialled.html),
the name for `WebSocket<MaybeTlsStream<TcpStream>>`. Every dependency whose types reach
this crate's public API is re-exported from the crate root — `s2_kit::tokio_tungstenite`,
`s2_kit::rustls`, `s2_kit::axum` — so nothing has to track a foreign major version by hand.

The certificate is verified against the platform trust store. For a LAN peer whose
certificate was pinned during pairing, use `WebSocket::connect_with_policy` — see
[S2 Connect](@/docs/connect.md).

## Where next

- [Validation](@/docs/validation.md) — what the 65 rules check, and why some are warnings.
- [Sessions](@/docs/sessions.md) — acknowledgements, instructions and fleets.
- [S2 Connect](@/docs/connect.md) — discovery, pairing and session initiation.
