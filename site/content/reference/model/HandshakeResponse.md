+++
title = "HandshakeResponse"
description = "Establishes the S2 protocol version to use during this S2 session."
[extra]
generated = true
+++

Establishes the S2 protocol version to use during this S2 session.

The CEM sends this message after exchanging `Handshake`s with the RM.

| Field | Type | Required | Description |
|---|---|---|---|
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"HandshakeResponse"`. |
| `selected_protocol_version` | `string` | **yes** | The protocol version the CEM selected for this session. This version should be one that was included in the `supported_protocol_versions` field of the exchanged `Handshake`s. |
