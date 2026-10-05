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
