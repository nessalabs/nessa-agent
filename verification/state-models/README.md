# State atlas verification evidence

The app source reviewed for this expansion is
`77a6b620af4d94b234294f6d273fd74b3dfb44c7`. The separate renderer/checker is
`nessalabs/nessa-docs` at `29cff2787b05038e4b7893f026cc7fb1731e3664`.
The rich-text source review additionally uses the app's pinned UI revision,
`a32191c26ee27c2d8159ab6498a0f0530a93e3ea`.

Use Node 24.22.1 and a built renderer with its verified TLC 1.7.4 cache, then
run from the repository root:

```sh
export NESSA_DOCS_ROOT=/absolute/path/to/nessa-docs
pnpm docs:state:check
pnpm docs:state:coverage
pnpm docs:state:models
pnpm docs:state:probes > verification/state-models/probe-results.json
```

`probe-results.json` records each unmodified baseline, its exact spec/config
model digest, and the deliberately weakened in-memory controls. A control is
successful only when its expected invariant breaks and no rule is left
unchecked. A missing model/probe, parse error or unfinished check fails. These
mutants are never written to the authored models or their saved page records.
The command is optional and not provisioned by CI; see the
[setup and exit-status contract](../../scripts/state-docs/README.md).

The last positive results live on the owning pages as checker-written
`modelCheck`. The [feature map](../../docs/state/feature-verification.md) explains
direct/shared coverage and the non-lifecycle boundaries. Existing page source
checkpoints retain their earlier scope after partial narrative reviews.

The screenshots show the local Nessa Docs browser at 1440 × 1000 in Chrome
156.0.8078.74. A complete local browser pass opened 167 atlas pages and rendered
203 diagram figures without a page or diagram error. This qualifies the local
atlas rendering, not native Nessa, a hosted MCP right panel or provider behavior.

Models check their declared bounded design abstractions. No fairness or eventual
completion is inferred, and successful design checks do not prove correspondence
to production code, physical erasure, cryptography, platform behavior or external
service availability. Product execution suites have separate evidence.
