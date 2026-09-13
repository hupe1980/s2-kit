+++
title = "SelectControlType"
description = "Activates a Control Type for this session."
[extra]
generated = true
+++

Activates a Control Type for this session.

This message is sent by the CEM to activate a Control Type for this session. The activated control type must be supported by the Resource Manager (as indicated in `ResourceManagerDetails` received by the CEM).

| Field | Type | Required | Description |
|---|---|---|---|
| `control_type` | `ControlType` | **yes** | The Control Type to activate. |
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"SelectControlType"`. |
