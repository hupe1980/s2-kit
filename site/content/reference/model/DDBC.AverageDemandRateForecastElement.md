+++
title = "DDBC.AverageDemandRateForecastElement"
description = "The DDBC.AverageDemandRateForecastElement type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `demand_rate_expected` | `float` | **yes** | The most likely value for the demand rate; the expected increase or decrease of the fill_level per second |
| `demand_rate_lower_68PPR` | `float` | no | The lower limit of the range with a 68 % probability that the demand rate is within that range |
| `demand_rate_lower_95PPR` | `float` | no | The lower limit of the range with a 95 % probability that the demand rate is within that range |
| `demand_rate_lower_limit` | `float` | no | The lower limit of the range with a 100 % probability that the demand rate is within that range |
| `demand_rate_upper_68PPR` | `float` | no | The upper limit of the range with a 68 % probability that the demand rate is within that range |
| `demand_rate_upper_95PPR` | `float` | no | The upper limit of the range with a 95 % probability that the demand rate is within that range |
| `demand_rate_upper_limit` | `float` | no | The upper limit of the range with a 100 % probability that the demand rate is within that range |
| `duration` | `Duration` | **yes** | Duration of the element |
