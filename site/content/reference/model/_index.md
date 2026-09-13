+++
title = "Data model"
description = "Every type of the S2 data model — all 36 messages and their components — with the field-by-field descriptions from the standard's own documentation."
weight = 30
[extra]
generated = true
+++

Generated from the S2 documentation project's `structured-documentation`, which
         that project calls "the single source of truth for other places where the data
         model is documented".

         Where this crate's own API documentation differs from the text below, the
         difference is deliberate and carries an `E` number in the
         [errata](@/docs/errata.md).

         | Type | Fields |
|---|---|
| [`Commodity`](@/reference/model/Commodity.md) | 4 |
| [`CommodityQuantity`](@/reference/model/CommodityQuantity.md) | 10 |
| [`ControlType`](@/reference/model/ControlType.md) | 7 |
| [`Currency`](@/reference/model/Currency.md) | 100 |
| [`DDBC.ActuatorDescription`](@/reference/model/DDBC.ActuatorDescription.md) | 6 |
| [`DDBC.ActuatorStatus`](@/reference/model/DDBC.ActuatorStatus.md) | 7 |
| [`DDBC.AverageDemandRateForecast`](@/reference/model/DDBC.AverageDemandRateForecast.md) | 4 |
| [`DDBC.AverageDemandRateForecastElement`](@/reference/model/DDBC.AverageDemandRateForecastElement.md) | 8 |
| [`DDBC.Instruction`](@/reference/model/DDBC.Instruction.md) | 8 |
| [`DDBC.OperationMode`](@/reference/model/DDBC.OperationMode.md) | 6 |
| [`DDBC.SystemDescription`](@/reference/model/DDBC.SystemDescription.md) | 6 |
| [`DDBC.TimerStatus`](@/reference/model/DDBC.TimerStatus.md) | 5 |
| [`Duration`](@/reference/model/Duration.md) | 0 |
| [`EnergyManagementRole`](@/reference/model/EnergyManagementRole.md) | 2 |
| [`FRBC.ActuatorDescription`](@/reference/model/FRBC.ActuatorDescription.md) | 6 |
| [`FRBC.ActuatorStatus`](@/reference/model/FRBC.ActuatorStatus.md) | 7 |
| [`FRBC.FillLevelTargetProfile`](@/reference/model/FRBC.FillLevelTargetProfile.md) | 4 |
| [`FRBC.FillLevelTargetProfileElement`](@/reference/model/FRBC.FillLevelTargetProfileElement.md) | 2 |
| [`FRBC.Instruction`](@/reference/model/FRBC.Instruction.md) | 8 |
| [`FRBC.LeakageBehaviour`](@/reference/model/FRBC.LeakageBehaviour.md) | 4 |
| [`FRBC.LeakageBehaviourElement`](@/reference/model/FRBC.LeakageBehaviourElement.md) | 2 |
| [`FRBC.OperationMode`](@/reference/model/FRBC.OperationMode.md) | 4 |
| [`FRBC.OperationModeElement`](@/reference/model/FRBC.OperationModeElement.md) | 4 |
| [`FRBC.StorageDescription`](@/reference/model/FRBC.StorageDescription.md) | 6 |
| [`FRBC.StorageStatus`](@/reference/model/FRBC.StorageStatus.md) | 3 |
| [`FRBC.SystemDescription`](@/reference/model/FRBC.SystemDescription.md) | 5 |
| [`FRBC.TimerStatus`](@/reference/model/FRBC.TimerStatus.md) | 5 |
| [`FRBC.UsageForecast`](@/reference/model/FRBC.UsageForecast.md) | 4 |
| [`FRBC.UsageForecastElement`](@/reference/model/FRBC.UsageForecastElement.md) | 8 |
| [`Handshake`](@/reference/model/Handshake.md) | 4 |
| [`HandshakeResponse`](@/reference/model/HandshakeResponse.md) | 3 |
| [`ID`](@/reference/model/ID.md) | 0 |
| [`InstructionStatus`](@/reference/model/InstructionStatus.md) | 7 |
| [`InstructionStatusUpdate`](@/reference/model/InstructionStatusUpdate.md) | 5 |
| [`NumberRange`](@/reference/model/NumberRange.md) | 2 |
| [`OMBC.Instruction`](@/reference/model/OMBC.Instruction.md) | 7 |
| [`OMBC.OperationMode`](@/reference/model/OMBC.OperationMode.md) | 5 |
| [`OMBC.Status`](@/reference/model/OMBC.Status.md) | 6 |
| [`OMBC.SystemDescription`](@/reference/model/OMBC.SystemDescription.md) | 6 |
| [`OMBC.TimerStatus`](@/reference/model/OMBC.TimerStatus.md) | 4 |
| [`PEBC.AllowedLimitRange`](@/reference/model/PEBC.AllowedLimitRange.md) | 4 |
| [`PEBC.EnergyConstraint`](@/reference/model/PEBC.EnergyConstraint.md) | 8 |
| [`PEBC.Instruction`](@/reference/model/PEBC.Instruction.md) | 7 |
| [`PEBC.PowerConstraints`](@/reference/model/PEBC.PowerConstraints.md) | 7 |
| [`PEBC.PowerEnvelope`](@/reference/model/PEBC.PowerEnvelope.md) | 3 |
| [`PEBC.PowerEnvelopeConsequenceType`](@/reference/model/PEBC.PowerEnvelopeConsequenceType.md) | 2 |
| [`PEBC.PowerEnvelopeElement`](@/reference/model/PEBC.PowerEnvelopeElement.md) | 3 |
| [`PEBC.PowerEnvelopeLimitType`](@/reference/model/PEBC.PowerEnvelopeLimitType.md) | 2 |
| [`PPBC.EndInterruptionInstruction`](@/reference/model/PPBC.EndInterruptionInstruction.md) | 8 |
| [`PPBC.PowerProfileDefinition`](@/reference/model/PPBC.PowerProfileDefinition.md) | 6 |
| [`PPBC.PowerProfileStatus`](@/reference/model/PPBC.PowerProfileStatus.md) | 3 |
| [`PPBC.PowerSequence`](@/reference/model/PPBC.PowerSequence.md) | 5 |
| [`PPBC.PowerSequenceContainer`](@/reference/model/PPBC.PowerSequenceContainer.md) | 2 |
| [`PPBC.PowerSequenceContainerStatus`](@/reference/model/PPBC.PowerSequenceContainerStatus.md) | 5 |
| [`PPBC.PowerSequenceElement`](@/reference/model/PPBC.PowerSequenceElement.md) | 2 |
| [`PPBC.PowerSequenceStatus`](@/reference/model/PPBC.PowerSequenceStatus.md) | 6 |
| [`PPBC.ScheduleInstruction`](@/reference/model/PPBC.ScheduleInstruction.md) | 8 |
| [`PPBC.StartInterruptionInstruction`](@/reference/model/PPBC.StartInterruptionInstruction.md) | 8 |
| [`PowerForecast`](@/reference/model/PowerForecast.md) | 4 |
| [`PowerForecastElement`](@/reference/model/PowerForecastElement.md) | 2 |
| [`PowerForecastValue`](@/reference/model/PowerForecastValue.md) | 8 |
| [`PowerMeasurement`](@/reference/model/PowerMeasurement.md) | 4 |
| [`PowerRange`](@/reference/model/PowerRange.md) | 3 |
| [`PowerValue`](@/reference/model/PowerValue.md) | 2 |
| [`ReceptionStatus`](@/reference/model/ReceptionStatus.md) | 4 |
| [`ReceptionStatusValues`](@/reference/model/ReceptionStatusValues.md) | 6 |
| [`ResourceManagerDetails`](@/reference/model/ResourceManagerDetails.md) | 14 |
| [`RevokableObjects`](@/reference/model/RevokableObjects.md) | 17 |
| [`RevokeObject`](@/reference/model/RevokeObject.md) | 4 |
| [`Role`](@/reference/model/Role.md) | 2 |
| [`RoleType`](@/reference/model/RoleType.md) | 3 |
| [`SelectControlType`](@/reference/model/SelectControlType.md) | 3 |
| [`SessionRequest`](@/reference/model/SessionRequest.md) | 4 |
| [`SessionRequestType`](@/reference/model/SessionRequestType.md) | 2 |
| [`Timer`](@/reference/model/Timer.md) | 3 |
| [`Transition`](@/reference/model/Transition.md) | 8 |
