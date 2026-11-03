# State documentation checks

This directory composes the separate [Nessa Docs](https://github.com/nessalabs/nessa-docs)
catalog and TLC service for this repository. It owns command execution and exit
status. The renderer owns Markdown/model validation, download pins, cache,
state/time bounds, counterexample parsing and result serialization. No app,
gateway, provider account or `nessa_ui` checkout is required.

Use Node 22.13+ in 22.x, 24.x or 26+, matching the renderer. In a separate
`nessa-docs` checkout, run `npm ci` and `npm run build:server`. Set its absolute
path from this repository:

```sh
export NESSA_DOCS_ROOT=/absolute/path/to/nessa-docs
pnpm docs:state:check
pnpm docs:state:coverage
pnpm docs:state:test
pnpm docs:state:models --install
pnpm docs:state:models
pnpm docs:state:probes
pnpm docs:state:models --write
```

`docs:state:check` validates the entire catalog, including Mermaid bindings and
linked files. It does not install or run Java. It reports existing warnings and
fails catalog errors. `docs:state:models` first does that check, discovers each
page's linked model, and runs them sequentially through the canonical service.
It does not execute the program described by a chart. Bare Node works too:
`node scripts/state-docs/check.mjs --models`.

`docs:state:coverage` also accounts for each accepted page. Direct model metadata
comes from the catalog; a shared model must be linked in the page's `Model coverage`
section. Stateful leaf pages and stateful parents without a direct/shared owner
fail this check. System/service/feature indexes, pure contracts, guides and
explicit external-source boundaries are reported separately and are not formal
passes. Mentioning an external provider in an exclusion does not hide a local
unmapped owner. Coverage status says the map is accounted for; run the models
separately to check their rules. The [feature map](../../docs/state/feature-verification.md)
is a reviewed snapshot of this report, not a second navigation manifest.

`--install` explicitly permits the renderer's checksum-verified TLC/Java cache
download. Without it, a missing cache is unavailable, never a pass. The cache
location and `NESSA_DOCS_TLC_HOME` override are owned by the
[renderer](https://github.com/nessalabs/nessa-docs/blob/main/docs/model-check.md).
`--write` saves finished results into each owning page's `modelCheck`, including
broken or unchecked results, using the renderer's writer. The default leaves
pages unchanged. A failed or cancelled run has no new record to save. Review
and commit deliberate result updates alongside their model and source checkpoint.

stdout is one JSON document; exit 0 means the catalog is valid and every requested
model passed, 1 means a catalog error or broken rule, and 2 means invalid usage,
unavailable setup, a failed/cancelled/bounded run or no linked models. Errors can
coexist with broken rules; a detected broken rule keeps exit 1. Partial runs
remain visible in JSON. SIGINT/SIGTERM cancel the current run.

`check.test.mjs` runs argument/setup cases on bare Node. When `NESSA_DOCS_ROOT`
is set it also exercises the actual catalog against synthetic projects. Set
`NESSA_STATE_TEST_MODELS=1` after installation to run actual TLC positive and
broken-guard controls; it never downloads automatically. These checks are opt-in
and are not added to the frontend or CI gates, which do not provision the renderer.

The [feature verification map](../../docs/state/feature-verification.md) links all
model owners and their exclusions across app features. `coverage.test.mjs` also
checks that unrelated links, stale evidence and lifecycle pages with children
cannot be mistaken for a completed model map.


`docs:state:probes` first requires each original model to pass, then checks the
in-memory safety weakenings in `probes/`. Every direct model must have at least
one probe; a missing owner, unknown owner, unchanged mutation, parse error,
unexpected invariant, timeout or unchecked rule is a non-pass. The command
never installs the checker and never writes intentionally broken models or
results to the project. Its JSON names the baseline and expected violations;
progress goes to stderr. Exit 0 means the originals passed and all controls
broke their expected rules, 1 means an assertion failed, and 2 means setup or
execution was unavailable. This is design-test evidence, not a product fault
injection or a proof of the implementation.
