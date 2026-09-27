# Agent credentials

`nessa-agent-credentials` is the pure shared domain language for credentials
that Nessa gives local coding agents. It contains no keychain, environment,
filesystem, serialization, provider, or runtime code.

```text
gateway source ──constructs──▶ AgentCredential + CredentialNamespace
                                      ▲
                         future consumers share these values
```

Arrows show adapters constructing or consuming immutable values. Each caller
owns its own application port. The crate owns only:

- `CredentialAgent`, the closed set of agents with Nessa-managed credentials;
- `AgentCredential`, private validated text and its API-key/OAuth meaning;
- `CredentialNamespace`, the stage and instance account scope accepted by
  credential adapters.

The canonical Security.framework service and per-agent item names remain
gateway infrastructure data in `protocol/defaults/agent-credentials.json`. Provider
environment variables remain provider infrastructure. Adding either concern
here would point the domain outwards.
