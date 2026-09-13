+++
title = "DDBC.SystemDescription"
description = "The DDBC.SystemDescription type of the S2 energy flexibility standard: its fields, their types, and which are required."
[extra]
generated = true
+++

| Field | Type | Required | Description |
|---|---|---|---|
| `actuators` | `DDBC.ActuatorDescription[]` | **yes** | List of all available actuators in the system. Must contain at least one `DDBC.ActuatorAggregated`. |
| `message_id` | `ID` | **yes** | ID of this message |
| `message_type` | `string` | **yes** |  |
| `present_demand_rate` | `NumberRange` | **yes** | Present demand rate that needs to be satisfied by the system |
| `provides_average_demand_rate_forecast` | `boolean` | **yes** | Indicates whether the Resource Manager could provide a demand rate forecast through the `DDBC.AverageDemandRateForecast`. |
| `valid_from` | `string` | **yes** | Moment this `DDBC.SystemDescription` starts to be valid. If the system description is immediately valid, the DateTimeStamp should be now or in the past. |
