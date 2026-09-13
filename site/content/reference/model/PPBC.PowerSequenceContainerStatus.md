+++
title = "PPBC.PowerSequenceContainerStatus"
description = "The PPBC.PowerSequenceContainerStatus type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `power_profile_id` | `ID` | **yes** | ID of the `PPBC.PowerProfileDefinition` of which the data element ‘sequence_container_id’ refers to. |
| `progress` | `Duration` | no | Time that has passed since the selected sequence has started. A value must be provided, unless no sequence has been selected or the selected sequence hasn’t started yet. |
| `selected_sequence_id` | `ID` | no | ID of selected `PPBC.PowerSequence`. When no ID is given, no sequence was selected yet. |
| `sequence_container_id` | `ID` | **yes** | ID of the `PPBC.PowerSequenceContainer` this `PPBC.PowerSequenceContainerStatus` provides information about. |
| `status` | `PPBC.PowerSequenceStatus` | **yes** | Status of the selected `PPBC.PowerSequence` |
