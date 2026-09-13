+++
title = "Handshake"
description = "Message sent to initialize an S2 session."
[extra]
generated = true
+++

Message sent to initialize an S2 session.

When starting a new S2 session, `Handshake` should be the first message sent from both sides. The CEM should then send a `HandshakeResponse` to establish the S2 protocol version used in the session. The handshaking process should always occur at the start of a session, even if the specific CEM and RM already have many succesfull S2 sessions behind them.

| Field | Type | Required | Description |
|---|---|---|---|
| `message_id` | `ID` | **yes** | ID of this message. |
| `message_type` | `string` | **yes** | The string `"Handshake"`. |
| `role` | `EnergyManagementRole` | **yes** | The role of the sender of this message. |
| `supported_protocol_versions` | `string[]` | no | Protocol versions supported by the sender of this message. This field is mandatory for the RM, but optional for the CEM. |
