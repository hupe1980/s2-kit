+++
title = "OMBC.Status"
description = "The OMBC.Status type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `active_operation_mode_id` | `ID` | **yes** | ID of the active `OMBC.OperationMode`. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `operation_mode_factor` | `float` | **yes** | The number indicates the factor with which the `OMBC.OperationMode` should be configured. The factor should be greater than or equal than 0 and less or equal to 1. |
| `previous_operation_mode_id` | `ID` | no | ID of the `OMBC.OperationMode` that was previously active. This value shall always be provided, unless the active `OMBC.OperationMode` is the first `OMBC.OperationMode` the Resource Manager is aware of. |
| `transition_timestamp` | `string` | no | Time at which the transition from the previous `OMBC.OperationMode` to the active `OMBC.OperationMode` was initiated. This value shall always be provided, unless the active `OMBC.OperationMode` is the first `OMBC.OperationMode` the Resource Manager is aware of. |
