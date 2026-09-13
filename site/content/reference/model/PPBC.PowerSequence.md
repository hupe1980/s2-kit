+++
title = "PPBC.PowerSequence"
description = "The PPBC.PowerSequence type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition_only` | `boolean` | **yes** | Indicates if this `PPBC.PowerSequence` may only be used during an abnormal condition |
| `elements` | `PPBC.PowerSequenceElement[]` | **yes** | List of `PPBC.PowerSequenceElements`. Shall contain at least one element. Elements must be placed in chronological order. |
| `id` | `ID` | **yes** | ID of the `PPBC.PowerSequence`. Must be unique in the scope of the `PPBC.PowerSequnceContainer` in which it is used. |
| `is_interruptible` | `boolean` | **yes** | Indicates whether the option of pausing a sequence is available. |
| `max_pause_before` | `Duration` | no | The maximum duration for which a device can be paused between the end of the previous running sequence and the start of this one |
