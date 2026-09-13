+++
title = "FRBC.ActuatorStatus"
description = "The FRBC.ActuatorStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `active_operation_mode_id` | `ID` | **yes** | ID of the `FRBC.OperationMode` that is presently active. |
| `actuator_id` | `ID` | **yes** | ID of the actuator this messages refers to |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `operation_mode_factor` | `float` | **yes** | The number indicates the factor with which the `FRBC.OperationMode` is configured. The factor should be greater than or equal than 0 and less or equal to 1. |
| `previous_operation_mode_id` | `ID` | no | ID of the `FRBC.OperationMode` that was active before the present one. This value shall always be provided, unless the active `FRBC.OperationMode` is the first `FRBC.OperationMode` the Resource Manager is aware of. |
| `transition_timestamp` | `string` | no | Time at which the transition from the previous `FRBC.OperationMode` to the active `FRBC.OperationMode` was initiated. This value shall always be provided, unless the active `FRBC.OperationMode` is the first `FRBC.OperationMode` the Resource Manager is aware of. |
