+++
title = "DDBC.ActuatorStatus"
description = "The DDBC.ActuatorStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `active_operation_mode_id` | `ID` | **yes** | The operation mode that is presently active for this actuator. |
| `actuator_id` | `ID` | **yes** | ID of the actuator this messages refers to |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `operation_mode_factor` | `float` | **yes** | The number indicates the factor with which the `DDBC.OperationMode` is configured. The factor should be greater than or equal to 0 and less or equal to 1. |
| `previous_operation_mode_id` | `ID` | no | ID of the DDBC,OperationMode that was active before the present one. This value shall always be provided, unless the active `DDBC.OperationMode` is the first `DDBC.OperationMode` the Resource Manager is aware of. |
| `transition_timestamp` | `string` | no | Time at which the transition from the previous `DDBC.OperationMode` to the active `DDBC.OperationMode` was initiated. This value shall always be provided, unless the active `DDBC.OperationMode` is the first `DDBC.OperationMode` the Resource Manager is aware of. |
