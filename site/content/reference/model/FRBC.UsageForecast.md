+++
title = "FRBC.UsageForecast"
description = "The FRBC.UsageForecast type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `elements` | `FRBC.UsageForecastElement[]` | **yes** | Further elements that model the profile. There shall be at least one element. Elements must be placed in chronological order. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `start_time` | `string` | **yes** | Time at which the `FRBC.UsageForecast` starts. |
