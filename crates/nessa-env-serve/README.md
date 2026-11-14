# Env serve

The environment role of the one binary. A gateway runs the copy of its own
build it installed on a host as `nessa env serve` over `ssh`, or the host dials
its gateway, and the two speak lease frames. This side starts and stops the
agent harnesses those leases run, runs the commands they ask for, keeps its own
audit of them, and keeps no conversation.

```text
nessa-server ──▶ nessa-env-serve ──▶ nessa-protocol (lease frames), nessa-sdk
                                └──▶ nessa-auth, nessa-client-core, nessa-device-pairing,
                                     nessa-local-storage, nessa-timer, shepherd
```

Arrows are compile-time dependencies. nessa-server imports it as
`nessa_env_serve`. The command line (`env serve`, `env dial`, `env grant`)
stays in the server's composition, and so does the lease protocol's
fingerprint in `env/config.rs`, which names every source of the contract here:
a change to any file in `src/` changes the protocol, so builds that differ are
refused at the hello.

Everything is Unix only, as launching a harness is, except `machine.rs`, which
`devices.list` also reads on every platform.

| Path | Responsibility |
| --- | --- |
| `src/application/` | The frame loop (`serve.rs`), lease policy, grants, approval and enrollment of a dialing host, redial, artifacts and fetches. |
| `src/domain/` | What a leasing peer is granted, and the terms. |
| `src/infrastructure/` | The harness launcher, lease ledger, host lock, grants file and its audit, the Cedar lease policy, outbox, inbox, commands, dial, pairing and self-update. |
| `src/install.rs` | Where this build lives on a host, and the commands that put it there. |
| `src/machine.rs` | What a machine has (`MachineProbe`), read within a bound. |
| `tests/` | Unit tests, each loaded by its source file. |

The `test-support` feature exposes what nessa-server's tests drive directly: a
grants file whose replacement can be substituted, its unsettled `change`, and
the `Asked` and `EndCause` a serving host reports.
