+++
title = "FRBC.FillLevelTargetProfileElement"
description = "The FRBC.FillLevelTargetProfileElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `duration` | `Duration` | **yes** | The duration of the element. |
| `fill_level_range` | `NumberRange` | **yes** | The target range in which the fill_level must be for the time period during which the element is active. The start of the range must be smaller or equal to the end of the range. The CEM must take best-effort actions to proactively achieve this target. |
