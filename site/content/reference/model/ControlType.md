+++
title = "ControlType"
description = "Describes a resource's control type(s)."
[extra]
generated = true
+++

Describes a resource's control type(s).

Control types describe how a resource is flexible. Typically, an RM will implement one control type, while a CEM will implement all five control types.


| Value | Description |
|---|---|
| `DEMAND_DRIVEN_BASED_CONTROL` | Demand Driven Based Control, used for devices which need to match a given demand of something, but are flexible in which way they satisfy this demand. |
| `FILL_RATE_BASED_CONTROL` | Fill Rate Based Control, used for devices which can store or buffer energy in some form. |
| `NOT_CONTROLABLE` | `NOT_CONTROLABLE` is used when no control is possible. Resources of this type can still provide measurements and forecast. |
| `NO_SELECTION` | Identifier that is to be used if no control type is or has been selected. |
| `OPERATION_MODE_BASED_CONTROL` | Operation Mode Based Control, used for devices which can adjust their power producing or consuming behavior, without constraints regarding the duration of the adjustment. |
| `POWER_ENVELOPE_BASED_CONTROL` | Power Envelope Base Control, used for devices of which the power producing or consuming behavior cannot be controlled, but can be limited in some way. |
| `POWER_PROFILE_BASED_CONTROL` | Power Profile Based Control, used for devices which have to perform a certain tasks, but that are flexible in when their tasks can be executed. |
