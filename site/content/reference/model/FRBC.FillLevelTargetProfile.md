+++
title = "FRBC.FillLevelTargetProfile"
description = "The FRBC.FillLevelTargetProfile type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `elements` | `FRBC.FillLevelTargetProfileElement[]` | **yes** | List of different fill levels that have to be targeted within a given duration. There shall be at least one element. Elements must be placed in chronological order. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `start_time` | `string` | **yes** | Time at which the `FRBC.FillLevelTargetProfile` starts. |
