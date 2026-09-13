+++
title = "Sessions"
description = "The S2 session engines in s2-kit: synchronous acknowledgements that cannot deadlock, resolved instructions, validity windows and timers, and driving a whole fleet from one timer."
weight = 30
+++

S2 is not a request/response protocol. Both sides send when they have something to say,
every message must be acknowledged, and a great deal of state accumulates: which control
type is active, which descriptions are in force, which instructions are outstanding, which
timers block which transitions.

`RmSession` and `CemSession` hold that state. Both are built on one core, so a CEM and an
RM from this crate can be wired back to back in memory — which is how the standard's own
documented conversations are tested.

## Acknowledgements cannot deadlock

S2 requires a `ReceptionStatus` for every message except a `ReceptionStatus`, and getting
that wrong is the ecosystem's most common bug. The upstream issue tracker is explicit
about the deadlocks that follow when an implementation *awaits* the acknowledgement it
owes.

s2-kit chooses the status **inside `handle_text`**, synchronously, from the codec, the
state table and the validator. Nothing is held across an `await`, so the deadlock cannot
happen. Every message sent enters a ledger with a deadline; the matching acknowledgement
resolves it, and a deadline that passes produces an **event**, not a stall.

## Instructions arrive resolved

An S2 instruction names an operation mode by identifier and gives a factor between zero
and one. Turning that into watts needs the system description that was sent earlier —
which is why every Resource Manager in the wild writes the same lookup by hand.

The engine does it once:

```text
FRBC.Instruction instr0 on actuator1 → Charging (charge) at factor 0.6,
  ElectricPower3PhaseSymmetric 3000 W, filling at 0.0015/s
```

…and says when it *cannot* be carried out:

```text
… ; blocked by Minimum discharge time (cooldown)
```

## What the engine remembers

- **The state table.** `S2C` says what may be sent when. The engine refuses outbound
  messages at the call site and answers inbound ones with `INVALID_CONTENT`.
- **Validity windows.** A description with a `valid_from` in the future is scheduled, and
  becomes effective at the right moment — `poll_timeout` returns that moment.
- **Timers.** A blocked transition is reported as blocked, with the timer's name. Timers
  are keyed by **actuator and timer**: the identifier is unique only within the actuator
  that declares it, so two actuators may both call one `timer1`.
- **Revocation.** Withdrawn objects stop being valid references. A system description has
  no `id`, so its `message_id` stands in — withdrawing one you have already replaced
  leaves the replacement alone.

All of it bounded, because an embedded target cannot grow a map for ever: 4096 tracked
identifiers, and 16 descriptions queued for a future `valid_from` — those entries are whole
messages, not identifiers.

## Driving a fleet

A Customer Energy Manager talks to every flexible device in a building. Each session has
its own deadlines, and one task per session turns a hundred devices into a hundred stacks
and a hundred timers.

A `SessionSet` answers the only question an event loop actually has:

```rust
use s2_kit::prelude::*;
use s2_kit::session::SessionSet;

let mut fleet: SessionSet<String, CemSession> = SessionSet::new();
fleet.insert("battery".into(), CemSession::new(CemConfig::default()));
fleet.insert("heatpump".into(), CemSession::new(CemConfig::default()));
fleet.open_all(Timestamp::now());

// One deadline for the whole fleet, and one sweep that serves every session due.
if let Some(deadline) = fleet.poll_timeout() {
    fleet.handle_timeout(deadline);
}
for (device, out) in fleet.drain_transmit() {
    let _ = (device, out);
}
```

Both roles satisfy the same `Session` trait, so a gateway fronting many devices and a
manager driving many are the same code.

## Numbers for whoever runs the fleet

An application reacts to events. An operator asks a different question — *which gateway is
being refused, and how slow is the slowest peer?* — and the only place the answer exists is
the engine that saw it.

```rust,ignore
let stats = session.stats();
metrics.gauge("s2.refused", stats.refused);
metrics.gauge("s2.nacked", stats.nacked);
metrics.gauge("s2.ack_timeouts", stats.ack_timeouts);
// The slowest round trip, not the mean: a mean hides the one peer that takes four seconds.
metrics.gauge("s2.ack_latency_max_ms", stats.ack_latency_max_ms);
```

Counters rather than a `Metrics` trait, because a hook you have to implement to learn a
number the session already has is a hook. The *sum* of the round trips is what is stored —
a sum can be added up across a fleet and a mean cannot — with `mean_ack_latency_ms()`
derived from it.

## An idle session goes quiet

A session that is never spoken to again must reach a state where it asks for nothing —
`poll_timeout` returns `None`. An engine that always wants waking turns an idle device
into a busy loop, and an engine whose deadlines go backwards turns one into a spin. Both
are tested directly, by driving each engine from nothing but its own clock.

## Performance

On an M-series laptop, per message:

| | small (`FRBC.StorageStatus`) | large (10-mode `FRBC.SystemDescription`) |
|---|---|---|
| decode | 0.40 µs | 6.9 µs |
| validate | 0.07 µs | 5.4 µs |
| encode | 0.09 µs | 2.0 µs |
| **`handle_text` end to end** | **1.1 µs** | |

A full handshake is 12 µs, so a thousand reconnecting devices cost twelve milliseconds of
CPU.
