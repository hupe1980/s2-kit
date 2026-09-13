+++
title = "ReceptionStatusValues"
description = "Describes the reception status of a message."
[extra]
generated = true
+++

Describes the reception status of a message.

`OK` is used for messages which are received successfully, the other variants are used when an issue was encountered processing the message.


| Value | Description |
|---|---|
| `INVALID_CONTENT` | Message contents is invalid (e.g. contains a non-existing ID). Consequence: message is ignored, proceed if possible. |
| `INVALID_DATA` | Message not understood (e.g. not valid JSON, no message_id found). Consequence: message is ignored, proceed if possible. |
| `INVALID_MESSAGE` | Message was not according to schema. Consequence: message is ignored, proceed if possible.ß |
| `OK` | Message processed normally. Consequence: proceed normally. |
| `PERMANENT_ERROR` | Receiver encountered an error which it cannot recover from. Consequence: disconnect. |
| `TEMPORARY_ERROR` | Receiver encountered an error. Consequence: try to send to message again. |
