# Shared names and bounds

Values more than one Nessa crate must agree on: the provider name each agent
binding records, the bounds on what a lease's environment publishes and
fetches, and the sandbox profile an environment enforces.

```text
nessa-sdk ──▶ nessa-types ◀── nessa-protocol
```

Arrows are compile-time dependencies.

**Admission rule.** No IO, no runtime, no dependencies. A value belongs here
only when the SDK and the protocol both name it, so neither depends on the
other to agree. The rule that checks a value stays with the crate that owns
it; this crate only names it. `PACKAGE_DENYLISTS` in
`scripts/architecture/rust-dependency-graphs.mjs` refuses `nessa-sdk`,
`nessa-protocol` and `tokio` in its graph.
