+++
title = "FRBC.LeakageBehaviour"
description = "The FRBC.LeakageBehaviour type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `elements` | `FRBC.LeakageBehaviourElement[]` | **yes** | List of elements that model the leakage behaviour of the buffer. The fill_level_ranges of the elements must be contiguous. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `valid_from` | `string` | **yes** | Moment this `FRBC.LeakageBehaviour` starts to be valid. If the `FRBC.LeakageBehaviour` is immediately valid, the DateTimeStamp should be now or in the past. |
