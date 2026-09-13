+++
title = "PPBC.PowerProfileDefinition"
description = "The PPBC.PowerProfileDefinition type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `end_time` | `string` | **yes** | Indicates when the last `PPBC.PowerSequence` shall be finished at the latest |
| `id` | `ID` | **yes** | ID of the `PPBC.PowerProfileDefinition`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `power_sequences_containers` | `PPBC.PowerSequenceContainer[]` | **yes** | The `PPBC.PowerSequenceContainers` that make up this `PPBC.PowerProfileDefinition`. There shall be at least one `PPBC.PowerSequenceContainer` that includes at least one `PPBC.PowerSequence`. `PPBC.PowerSequenceContainers` must be placed in chronological order. |
| `start_time` | `string` | **yes** | Indicates the first possible time the first `PPBC.PowerSequence` could start |
