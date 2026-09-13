+++
title = "PPBC.PowerSequenceStatus"
description = "The PPBC.PowerSequenceStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++


| Value | Description |
|---|---|
| `ABORTED` | The selected `PPBC.PowerSequence` was aborted by the device and will not continue |
| `EXECUTING` | The selected `PPBC.PowerSequence` is currently being executed |
| `FINISHED` | The selected `PPBC.PowerSequence` was executed and finished successfully |
| `INTERRUPTED` | The selected `PPBC.PowerSequence` is being executed, but is currently interrupted and will continue afterwards |
| `NOT_SCHEDULED` | No `PPBC.PowerSequence` within the `PPBC.PowerSequenceContainer` is scheduled |
| `SCHEDULED` | The selected `PPBC.PowerSequence` is scheduled to be executed in the future |
