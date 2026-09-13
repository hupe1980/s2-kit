+++
title = "FRBC.SystemDescription"
description = "The FRBC.SystemDescription type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `actuators` | `FRBC.ActuatorDescription[]` | **yes** | Details of all Actuators. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `storage` | `FRBC.StorageDescription` | **yes** | Details of the storage. |
| `valid_from` | `string` | **yes** | Moment this `FRBC.SystemDescription` starts to be valid. If the system description is immediately valid, the DateTimeStamp should be now or in the past. |
