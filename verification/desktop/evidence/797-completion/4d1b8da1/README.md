# Completion verification at 4d1b8da1

Checked source: `4d1b8da1404ec7ad38a867cf1f24a771bc32b0ec`, assembled from
main `9e46516d07073797b302dcb14f920c1e8b97e066` and the published heads
#834 `d8ddee25`, #838 `4e88643c`, #823 `c0a9f910`, #859 `858eda29`,
#828 `f81db23a`. The UI pin is the checkout's `nessa-ui-revision`; each
package and font resolves inside this checkout. These evidence files do not
change the checked runtime source.

A fresh read-only combined integration review found no findings. It checked
prepared intent/Stop/receipt agreement, placement/resource/gesture lifetimes,
clock composition and verifier prefix/cleanup/registry behavior. Individual
PR reviews and their regression probes remain separately attributed.

## Executed checks

- Actual-App attachment replay: 40/40 across Chromium/WebKit, zero page errors.
  The four PNGs in this directory are current captures from that replay;
  `attachments.json` retains its exact scenario results.

- Full frontend gate: 4,563 tests / 343 files, plus format, lint, script tests,
  types, architecture, protocol and production build. The final repeat after the
  dependency ownership correction passed; short result lines are in `checks.log`.
- Normal signed-out real-gateway runner: MCP apps 16/16, gateway window 13/13,
  scripted scenarios 8/8, Chromium and WebKit. See `scripted-summary.md`.
- Production delivery: 14/14 checks; three fresh-page runs per engine/layout,
  calibrated 4× Chromium and unthrottled WebKit. Maximum frame 37.7/50 ms,
  active delivery 44.7/600 ms, held delivery 34.0/600 ms. Exact unrounded
  samples and calibration assertions are in `delivery.json`.

The gateway binary was rebuilt from this checkout's server source using a
private Cargo target and explicitly supplied through `MCP_LIVE_NESSA`.
The source's Rust crates are byte-identical to the fresh main/attachment
parent used for the preceding native server build. Supported-target CI
remains a separate PR gate; these local results do not substitute for it.

## Setup failure and correction

The first gateway run failed with font 403s: an old ignored
`node_modules/.pnpm` symlink resolved fonts in the primary checkout, beyond
Vite's permitted source roots. The old local dependency directory was
preserved under `/tmp/nessa-797-combined-borrowed-node-modules`; a clean
frozen-lockfile install made the virtual store and fonts local. The source,
font versions and console assertions stayed unchanged. Fresh typecheck and
the ordinary gateway runner then passed. No allowlist or timing budget was
weakened. Local failure log: `/tmp/nessa-final-main9e-scripted.log`.

This establishes the font rejection's cause and correction. It does not
claim a causal explanation for the older 154 ms performance sample, or for
the user's unidentified Fastify error window.
