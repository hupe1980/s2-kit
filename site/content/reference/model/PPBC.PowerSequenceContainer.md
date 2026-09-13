+++
title = "PPBC.PowerSequenceContainer"
description = "The PPBC.PowerSequenceContainer type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | `ID` | **yes** | ID of the `PPBC.PowerSequenceContainer`. Must be unique in the scope of the `PPBC.PowerProfileDefinition` in which it is used. |
| `power_sequences` | `PPBC.PowerSequence[]` | **yes** | List of alternative Sequences where one could be chosen by the CEM |
