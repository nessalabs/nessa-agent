---
id: credentials-values
title: Credential value contracts
kind: contract
status: implemented
summary: "These values describe a validated agent secret and where it belongs."
parent: credentials
sources:
  - crates/nessa-agent-credentials/src/domain/value_objects/agent_credential.rs
  - crates/nessa-agent-credentials/README.md
diagramLinks: {}
---

# Credential value contracts

These values describe a validated agent secret and where it belongs. They distinguish an API key from OAuth credentials and restrict the supported agent and storage namespace.

Creating one checks its shape. It does not save the secret, contact an agent, or prove the agent will accept it. Storage and readiness checks belong to the services using these values, so this page has no runtime statechart.



## Further reading

[Source](../../../../crates/nessa-agent-credentials/src/domain/value_objects/agent_credential.rs) · [Related source](../../../../crates/nessa-agent-credentials/README.md)

## Model coverage

Pure contract: immutable validated credential and namespace values have no persistence, network or revocation owner.

Storage and readiness belong to consuming services; constructor success does not prove a provider accepts the secret. This relationship applies only to the stated scope, not every step of this journey. This scoped mapping retains earlier source checkpoints and is not executed implementation-test evidence.
