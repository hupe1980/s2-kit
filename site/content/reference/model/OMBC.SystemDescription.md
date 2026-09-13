+++
title = "OMBC.SystemDescription"
description = "The OMBC.SystemDescription type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `operation_modes` | `OMBC.OperationMode[]` | **yes** | `OMBC.OperationModes` available for the CEM in order to coordinate the device behaviour. |
| `timers` | `Timer[]` | **yes** | Timers that control when certain transitions can be made. |
| `transitions` | `Transition[]` | **yes** | Possible transitions to switch from one `OMBC.OperationMode` to another. |
| `valid_from` | `string` | **yes** | Moment this `OMBC.SystemDescription` starts to be valid. If the system description is immediately valid, the DateTimeStamp should be now or in the past. |
