+++
title = "ResourceManagerDetails"
description = "Describes various properties of an RM."
[extra]
generated = true
+++

Describes various properties of an RM.

The RM sends this message after the handshaking process has taken place to let the CEM know about its various property. Importantly, `available_control_types` lets the CEM know what control types the RM supports. The CEM will then usually respond with a `SelectControlType` message to select one of the supported control types.

| Field | Type | Required | Description |
|---|---|---|---|
| `available_control_types` | `ControlType[]` | **yes** | The control types supported by this Resource Manager. |
| `currency` | `Currency` | no | Currency to be used for all information regarding costs. Mandatory if cost information is published. |
| `firmware_version` | `string` | no | Version identifier of the firmware used in the device (provided by the manufacturer) |
| `instruction_processing_delay` | `Duration` | **yes** | The average time the combination of Resource Manager and HBES/BACS/SASS or (Smart) device needs to process and execute an instruction |
| `manufacturer` | `string` | no | The name of this resource's manufacturer. |
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"ResourceManagerDetails"`. |
| `model` | `string` | no | The name of the model of the device (provided by the manufacturer). |
| `name` | `string` | no | A human readable name given by the user of the resource. |
| `provides_forecast` | `boolean` | **yes** | Indicates whether the ResourceManager is able to provide `PowerForecast`s. |
| `provides_power_measurement_types` | `CommodityQuantity[]` | **yes** | Indicates for which CommodityQuantities this Resource Manager can provide measurements. |
| `resource_id` | `ID` | **yes** | Unique identifier of the Resource Manager. |
| `roles` | `Role[]` | **yes** | Describes which commodities the resource can consume, produce and/or store. |
| `serial_number` | `string` | no | The serial number of the device (provided by the manufacturer). |
