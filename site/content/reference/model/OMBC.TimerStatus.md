+++
title = "OMBC.TimerStatus"
description = "The OMBC.TimerStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `finished_at` | `string` | **yes** | Indicates when the Timer will be finished. If the DateTimeStamp is in the future, the timer is not yet finished. If the DateTimeStamp is in the past, the timer is finished. If the timer was never started, the value can be an arbitrary DateTimeStamp in the past. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `timer_id` | `ID` | **yes** | The ID of the timer this message refers to |
