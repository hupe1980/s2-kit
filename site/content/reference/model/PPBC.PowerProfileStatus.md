+++
title = "PPBC.PowerProfileStatus"
description = "The PPBC.PowerProfileStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `sequence_container_status` | `PPBC.PowerSequenceContainerStatus[]` | **yes** | Array with status information for all `PPBC.PowerSequenceContainers` in the `PPBC.PowerProfileDefinition`. |
