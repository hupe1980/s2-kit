+++
title = "FRBC.LeakageBehaviourElement"
description = "The FRBC.LeakageBehaviourElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `fill_level_range` | `NumberRange` | **yes** | The fill level range for which this `FRBC.LeakageBehaviourElement` applies. The start of the range must be less than the end of the range. |
| `leakage_rate` | `float` | **yes** | Indicates how fast the momentary fill level will decrease per second due to leakage within the given range of the fill level. A positive value indicates that the fill level decreases over time due to leakage. |
