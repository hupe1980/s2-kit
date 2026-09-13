+++
title = "PEBC.Instruction"
description = "The PEBC.Instruction type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition` | `boolean` | **yes** | Indicates if this is an instruction during an abnormal condition. |
| `execution_time` | `string` | **yes** | Indicates the moment the execution of the instruction shall start. When the specified execution time is in the past, execution must start as soon as possible. |
| `id` | `ID` | **yes** | Identifier of this `PEBC.Instruction`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `power_constraints_id` | `ID` | **yes** | Identifier of the `PEBC.PowerConstraints` this `PEBC.Instruction` was based on. |
| `power_envelopes` | `PEBC.PowerEnvelope[]` | **yes** | The `PEBC.PowerEnvelope`(s) that should be followed by the Resource Manager. There shall be at least one `PEBC.PowerEnvelope`, but at most one `PEBC.PowerEnvelope` for each CommodityQuantity. |
