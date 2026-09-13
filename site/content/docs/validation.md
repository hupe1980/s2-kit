+++
title = "Validation"
description = "How s2-kit validates S2 messages: the three layers of checking, why a rule is an error or a warning, and how rule identifiers make a refusal greppable across a fleet."
weight = 20
+++

A JSON Schema says a message is *shaped* correctly. It cannot say that the actuator you
named exists, that a factor of 1.3 is outside the range the device published, or that the
operation mode you instructed belongs to a different actuator. Those are semantic rules,
and the standard states them in prose scattered across its documentation.

s2-kit turns that prose into a **numbered catalogue**: 61 rules, each quoting the sentence
it implements, each with a test that fires it.

## Three layers

Validation happens in three places, and which one refuses a message determines what the
peer is told:

| Layer | Checks | Reception status |
|---|---|---|
| **Codec** | is this JSON, of a known type, matching the schema? | `INVALID_DATA` / `INVALID_MESSAGE` |
| **Intra-message** | is this message self-consistent? | `INVALID_CONTENT` |
| **Cross-message** | is it consistent with what was said earlier? | `INVALID_CONTENT` |

The third needs memory. A rule that compares an instruction against the system description
the device sent ten minutes ago cannot fire on a message read from a file, so those rules
are marked as needing session context and the engines supply it.

## Representable is not valid

The Rust types accept everything the schema accepts — including a factor of 1.3 and a
289-element forecast. That is deliberate, for two reasons:

1. **A proxy must carry what it would refuse to send.** Forwarding a message with a value
   you dislike is more useful than dropping it.
2. **Answering `INVALID_CONTENT` means having decoded the message first.** You cannot tell
   a peer which rule it broke if your parser refused to build the value.

So the types accept, the validator judges, and the builders refuse. Three different jobs.

## Errors and warnings

A rule is an **error** only where the standard states a requirement plainly. Where it is
ambiguous, silent, or contradicts itself, the rule is a **warning** — and a warning never
refuses traffic.

This is not timidity. A validator that refuses the standard's own published examples is a
bug, not a feature, and that is not hypothetical: the heat-pump walkthrough in the S2
documentation sends zero-valued costs with no currency, while the currency field is
described as "mandatory if cost information is published". Refusing it would make this
crate stricter than the documentation it implements. That one is a warning, and it is
recorded in the [errata](@/docs/errata.md) as E17.

Of the 61 rules, 42 are errors and 19 are warnings.

## Three vantage points

Which rules can fire depends on what the validator can see.

| You have | Use | What fires |
|---|---|---|
| one message, in a file | `Context::empty()` | the intra-message rules: ranges, factors, contiguity, array bounds |
| one side of a live conversation | the session's own context — `RmSession`/`CemSession` do it for you | those, plus everything about what *this side received* |
| the whole conversation | `session::Analyzer` | all of them |

The third is the one worth knowing about. A session holds only its own half: a Resource
Manager's registry records what it *received*, so replaying a transcript through one would
check the instructions and know nothing about the descriptions they name — the descriptions
came from that side. `Analyzer` keeps one registry fed from both directions, so an
instruction naming an actuator nobody described is a finding rather than a blind spot.

```console
$ s2-kit replay session.s2log
   4 cem>rm FRBC.Instruction: S2-FRBC-003 at /actuator_id: nope is not an actuator of this system
   7 rm>cem FRBC.StorageStatus: never answered; no ReceptionStatus names m7
   9 cem>rm FRBC.Instruction: answered OK, s2-kit would answer InvalidContent (S2-NUM-004 at /operation_mode_factor: operation mode factor 1.3 is outside [0, 1])
12 line(s), 9 answered, checked against 61 rules: FAILED
```

Three kinds of finding, and only the first is a rule: a message that broke one, a message
**nobody answered** — which `S2J` requires and is the deadlock
[`s2-json#22`](https://github.com/flexiblepower/s2-json/issues/22) describes — and a
`ReceptionStatus` that differs from the one this crate would have sent, which is the interop
report in one line.

It decodes leniently and never stops: a frame that is not S2 at all is an `S2-MSG-004`
finding, not the end of the file.

Two warnings exist for a different reason: another implementation is stricter than the
standard. `s2-python` refuses an FRBC actuator description that does not publish a power
range for every commodity it claims, and one that repeats a commodity — neither required by
the schema. `S2-ACT-001` and `S2-ACT-002` warn you before a field trial does.

## What the identifier names

A rule's area names **what the rule is about**, not which control type reached it.
`previous_operation_mode_id` carries the same sentence in `FRBC.ActuatorStatus`,
`OMBC.Status` and `DDBC.ActuatorStatus`, so it is one rule, `S2-STATUS-001` — and an
OMBC-only device never sees an `S2-FRBC-…` identifier for a control type it does not
implement.

Three rules come from the session engines rather than the validator, because they describe
how a message *arrived*: `S2-MSG-004` (it did not decode), `S2-MSG-005` (a property the
schema does not define was pruned in lenient mode) and `S2-MSG-006` (your `InboundPolicy`
refused it). Three different things to count across a fleet, so three identifiers.

## Rule identifiers on the wire

Every failing `ReceptionStatus` this crate sends carries the rule that failed:

```json
{"message_type":"ReceptionStatus","subject_message_id":"mx","status":"INVALID_CONTENT",
 "diagnostic_label":"S2-FRBC-003 at /actuator_id: nope is not an actuator of this system"}
```

The identifier, a JSON pointer to the exact field, and a sentence. A peer's log becomes
greppable, and a support ticket becomes a rule number rather than "it didn't work".

## Refusing your own mistakes

The validator runs on the way out too, so a mistake is caught at the call site rather than
by the peer:

```rust,ignore
// −6 kW is outside the −4000..0 the inverter published.
let error = cem.instruct(too_much, now).unwrap_err();
// S2-PEBC-006 at /power_envelopes/0/power_envelope_elements/0/lower_limit:
//   -6000 is outside every allowed LowerLimit range for ElectricPowerL1
```

## From the command line

The validator has no opinion about what wrote the message, which makes it useful in other
implementations' CI:

```console
$ s2-kit validate message.json
message 1: S2-NUM-004 at /operation_mode_factor: operation mode factor 1.3 is outside [0, 1]
1 message(s) checked against 61 rules: FAILED
```

The full catalogue, with the source sentence for each rule, is the
[rule reference](@/reference/rules.md).
