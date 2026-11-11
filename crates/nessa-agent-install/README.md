# Agent install

Getting an agent's own pinned runtime onto this machine: choose the release
Nessa tested for this host, fetch it over HTTPS, check its digest, unpack it
into a private store, record what is installed, and reclaim what a newer pin
replaced.

```text
nessa-server ──▶ nessa-agent-install ──▶ nessa-local-storage
                                    └──▶ nessa-auth, nessa-protocol
```

Arrows are compile-time dependencies. nessa-server uses it as
`nessa_agent_install`, composes it (`composition/install_command.rs`,
`composition/installed_launch.rs`) and serves `agents.install` over the
product socket.

| Path | Responsibility |
| --- | --- |
| `src/domain/` | Pins, release contents, host platform, install attempts and reclamation rules. No disk or network. |
| `src/application/` | The install and reclamation flows, and the ports they drive (`ports.rs`, `gateway.rs`). |
| `src/infrastructure/` | Pinned releases compiled in from `data/agent-releases.json`, HTTPS archives, the managed runtime store, and the durable audit and delivery journals. |
| `data/agent-releases.json` | The tested pins, written by `scripts/agents/pin-agents.mjs`. |
| `tests/` | Unit tests, included with `#[path]`; `support.rs` is the shared fixtures, also built for nessa-server's tests under the `test-support` feature. |
