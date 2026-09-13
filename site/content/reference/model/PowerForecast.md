+++
title = "PowerForecast"
description = "Forecast of the power values for a future period of time."
[extra]
generated = true
+++

Forecast of the power values for a future period of time.

| Field | Type | Required | Description |
|---|---|---|---|
| `elements` | `PowerForecastElement[]` | **yes** | The elements describing the forecast. This must contain at least one element, and elements must be placed in chronological order. |
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"PowerForecast"`. |
| `start_time` | `string` | **yes** | Start time of time period that is covered by the profile. |
