+++
title = "FRBC.Instruction"
description = "The FRBC.Instruction type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `abnormal_condition` | `boolean` | **yes** | Indicates if this is an instruction during an abnormal condition. |
| `actuator_id` | `ID` | **yes** | ID of the actuator this instruction belongs to. |
| `execution_time` | `string` | **yes** | Indicates the moment the execution of the instruction shall start. When the specified execution time is in the past, execution must start as soon as possible. |
| `id` | `ID` | **yes** | ID of the instruction. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `operation_mode` | `ID` | **yes** | ID of the `FRBC.OperationMode` that should be activated. |
| `operation_mode_factor` | `float` | **yes** | The number indicates the factor with which the `FRBC.OperationMode` should be configured. The factor should be greater than or equal to 0 and less or equal to 1. |
