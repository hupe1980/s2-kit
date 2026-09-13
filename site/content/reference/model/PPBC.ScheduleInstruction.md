+++
title = "PPBC.ScheduleInstruction"
description = "The PPBC.ScheduleInstruction type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition` | `boolean` | **yes** | Indicates if this is an instruction during an abnormal condition |
| `execution_time` | `string` | **yes** | Indicates the moment the `PPBC.PowerSequence` shall start. When the specified execution time is in the past, execution must start as soon as possible. |
| `id` | `ID` | **yes** | ID of the Instruction. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `power_profile_id` | `ID` | **yes** | ID of the `PPBC.PowerProfileDefinition` of which the `PPBC.PowerSequence` is being selected and scheduled by the CEM. |
| `power_sequence_id` | `ID` | **yes** | ID of the `PPBC.PowerSequence` that is being selected and scheduled by the CEM. |
| `sequence_container_id` | `ID` | **yes** | ID of the `PPBC.PowerSequnceContainer` of which the `PPBC.PowerSequence` is being selected and scheduled by the CEM. |
