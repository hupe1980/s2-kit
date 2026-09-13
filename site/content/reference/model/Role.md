+++
title = "Role"
description = "Describes that a resource can consume, produce or store a commodity."
[extra]
generated = true
+++

Describes that a resource can consume, produce or store a commodity.

| Field | Type | Required | Description |
|---|---|---|---|
| `commodity` | `Commodity` | **yes** | The commodity being referred to. |
| `role` | `RoleType` | **yes** | Indicates whether the resourse can consume, produce or store the specified commodity. |
