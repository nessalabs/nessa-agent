# Manual canonical browser verification

`.github/workflows/canonical-browser-verification.yml` runs only on explicit
dispatch. Normal pull-request required checks are unchanged. The workflow must
exist on the default branch before GitHub accepts a dispatch; this draft does
not provide browser qualification merely by passing workflow validation.

Dispatch with `source` set to the reviewed full lowercase commit SHA. Leave
`base` empty to qualify that source tree. Dispatch a second time with the same
`source` and a pinned current-main `base` to qualify their integration. The
preparation script requires both commits to be reachable from this repository's
fetched branches, refuses a dirty checkout, and stops on merge conflict. It
records the workflow commit, repository, source, base and assembled tree in
`provenance.json` before running dependencies or the canonical suite. The
integration does not create or push a commit.

The Ubuntu runner uses the existing Node 24, Rust stable, read-only Rust cache
and lockfile-based pnpm setup. Playwright installs its pinned Chromium/WebKit
builds and official runtime dependencies. The command is unchanged:

```sh
pnpm test:e2e:scripted -- --channel bundled --evidence "$BROWSER_EVIDENCE"
```

This signed-out suite runs all three checks in development mode, in Chromium
and WebKit. It requires no provider credential or repository secret. The job
has `contents: read`, checkout credentials are not persisted, and no cache is
written by this workflow. Only reviewed same-repository code should be selected;
manual dispatch is not a sandbox for untrusted code on a privileged runner.

An always-run artifact step retains preparation/install/canonical logs,
provenance, per-engine JSON, screenshots, gateway logs and the canonical
`pr-summary.md` for seven days. Inspect both provenance and the canonical verdict;
missing capabilities and timeouts do not count as a pass. Native desktop/WebView
qualification remains separate from Playwright's browser engines.

Planning estimate per dispatch: 5–15 Ubuntu minutes warm, 20–40 minutes cold;
these are unmeasured estimates. The job stops at 60 minutes. Run source and
integration separately so their evidence and build artifacts cannot be confused.
