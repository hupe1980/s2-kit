+++
title = "FRBC.UsageForecastElement"
description = "The FRBC.UsageForecastElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `duration` | `Duration` | **yes** | Indicator for how long the given usage_rate is valid. |
| `usage_rate_expected` | `float` | **yes** | The most likely value for the usage rate; the expected increase or decrease of the fill_level per second. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_lower_68PPR` | `float` | no | The lower limit of the range with a 68 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_lower_95PPR` | `float` | no | The lower limit of the range with a 95 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_lower_limit` | `float` | no | The lower limit of the range with a 100 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_upper_68PPR` | `float` | no | The upper limit of the range with a 68 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_upper_95PPR` | `float` | no | The upper limit of the range with a 95 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
| `usage_rate_upper_limit` | `float` | no | The upper limit of the range with a 100 % probability that the usage rate is within that range. A positive value indicates that the fill level will decrease due to usage. |
