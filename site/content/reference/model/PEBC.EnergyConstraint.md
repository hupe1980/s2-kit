+++
title = "PEBC.EnergyConstraint"
description = "The PEBC.EnergyConstraint type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity_quantity` | `CommodityQuantity` | **yes** | Type of power quantity which applies to upper_average_power and lower_average_power |
| `id` | `ID` | **yes** | Identifier of this `PEBC.EnergyConstraints`. Must be unique in the scope of the Resource Manager, for at least the duration of the session between Resource Manager and CEM. |
| `lower_average_power` | `float` | **yes** | Lower average power within the time period given by valid_from and valid_until. If the duration is multiplied with this power value, then the associated lower energy content can be derived. This is the lowest amount of energy the resource will consume during that period of time. The Power Envelope created by the CEM must allow at least this much energy production (in case the number is negative). Must be greater than or equal to lower_average_power, and can be negative in case of energy production. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `upper_average_power` | `float` | **yes** | Upper average power within the time period given by valid_from and valid_until. If the duration is multiplied with this power value, then the associated upper energy content can be derived. This is the highest amount of energy the resource will consume during that period of time. The Power Envelope created by the CEM must allow at least this much energy consumption (in case the number is positive). Must be greater than or equal to lower_average_power, and can be negative in case of energy production. |
| `valid_from` | `string` | **yes** | Moment this `PEBC.EnergyConstraints` information starts to be valid |
| `valid_until` | `string` | **yes** | Moment until this `PEBC.EnergyConstraints` information is valid. |
