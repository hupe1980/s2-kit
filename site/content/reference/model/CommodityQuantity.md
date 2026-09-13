+++
title = "CommodityQuantity"
description = "Specifies the commodity and quantity a value is describing."
[extra]
generated = true
+++

Specifies the commodity and quantity a value is describing.

This enumeration is used to specify the commodity and quantity of an accompanying value. It's used in many places where S2 information is shared in a commodity-agnostic way; for example, in `PEBC.EnergyConstraint` it's used to specify what commodity/quantity the constraint is about.


| Value | Description |
|---|---|
| `ELECTRIC.POWER.3_PHASE_SYMMETRIC` | Electric power, described in Watt when power is equally shared among the three phases. Only applicable for 3 phase devices. A value refers to the total power on the three phases. |
| `ELECTRIC.POWER.L1` | Electric power, described in Watt on phase 1. If a device utilizes only one phase, it should always use L1. |
| `ELECTRIC.POWER.L2` | Electric power, described in Watt on phase 2. Only applicable for 3 phase devices. |
| `ELECTRIC.POWER.L3` | Electric power, described in Watt on phase 3. Only applicable for 3 phase devices. |
| `HEAT.FLOW_RATE` | Flow rate of heat carrying gas or liquid, described in liters per second. |
| `HEAT.TEMPERATURE` | Heat, described in degrees Celsius. |
| `HEAT.THERMAL_POWER` | Thermal power, described in Watt. |
| `HYDROGEN.FLOW_RATE` | Hydrogen gas flow rate, described in grams per second |
| `NATURAL_GAS.FLOW_RATE` | Natural gas flow rate, described in liters per second. |
| `OIL.FLOW_RATE` | Oil flow rate, described in liters per hour. |
