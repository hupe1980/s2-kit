+++
title = "DDBC.AverageDemandRateForecast"
description = "The DDBC.AverageDemandRateForecast type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `elements` | `DDBC.AverageDemandRateForecastElement[]` | **yes** | Elements of the profile. Elements must be placed in chronological order. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `start_time` | `string` | **yes** | Start time of the profile. |
