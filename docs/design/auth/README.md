# Authentication design references

This folder holds explanations, research, and detailed design material.
Implementation decisions and unfinished work are tracked in the ADR folders.

| Document | Purpose |
| --- | --- |
| [Cedar primer](cedar-primer.md) | How policy evaluation works, with examples and links to the implementation |
| [Scoped authentication design](scoped-authentication.md) | Original detailed plan and acceptance criteria, with current status and superseded assumptions identified |
| [Identity, tenancy, and cloud](identity-tenancy-and-cloud.md) | Proposed identity/provider direction and supporting research; broader integrations remain unimplemented |

## Decisions and work tracking

- [Done: ADR 0010 — local authentication](../../adr/done/0010-local-authentication.md).
- [Todo: ADR 0011 — authentication delivery](../../adr/todo/0011-authentication-delivery.md).
- [Todo: ADR 0007 — session protocol and bindings](../../adr/todo/0007-nessa-session-protocol-and-authorities.md).
- [Todo: ADR 0009 — harnesses and optional tools](../../adr/todo/0009-agent-harnesses-and-optional-tools.md).

## Using and reviewing the implementation

- [Local setup, token minting, and recovery](../../guides/local-auth.md).
- [Adversarial gateway review](../../reviews/local-auth-gateway.md).
- [Auth crate guide](../../../crates/nessa-auth/README.md).
- [Typed dependency injection](../dependency-injection.md).
