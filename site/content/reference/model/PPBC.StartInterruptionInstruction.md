+++
title = "PPBC.StartInterruptionInstruction"
description = "The PPBC.StartInterruptionInstruction type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition` | `boolean` | **yes** | Indicates if this is an instruction during an abnormal condition |
| `execution_time` | `string` | **yes** | Indicates the moment the `PPBC.PowerSequence` shall be interrupted. When the specified execution time is in the past, execution must start as soon as possible. |
| `id` | `ID` | **yes** | ID of the Instruction. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `power_profile_id` | `ID` | **yes** | ID of the `PPBC.PowerProfileDefinition` of which the `PPBC.PowerSequence` is being interrupted by the CEM. |
| `power_sequence_id` | `ID` | **yes** | ID of the `PPBC.PowerSequence` that the CEM wants to interrupt. |
| `sequence_container_id` | `ID` | **yes** | ID of the `PPBC.PowerSequnceContainer` of which the `PPBC.PowerSequence` is being interrupted by the CEM. |
