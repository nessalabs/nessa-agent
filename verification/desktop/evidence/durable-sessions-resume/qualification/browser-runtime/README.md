# Private browser runtime readiness

Local runtime check only. This does not qualify Nessa product behavior, browser
stress, native WKWebView, performance, credentials, or a live provider.

Chromium 153.0.8010.12 and WebKit 26.6 both launched headless, rendered a local
button/output document, clicked the button, and read `clicked`, with zero page
errors. See `final-smoke.json`. Each browser closes in `finally`; the smoke
command runs under `docker-init -s -- timeout 75s`. No persistent server was
started. No source or Cargo artifacts were changed.

## Launch from the repository

```sh
source /workspace/.setup/activate.sh
source /workspace/scratch/durable-browser-runtime/activate.sh
/usr/local/bin/docker-init -s -- timeout 75s node /workspace/scratch/durable-qualification/browser-runtime/smoke.cjs
```

For repository browser scripts use the same two sourced files and choose
`--channel bundled` where the script otherwise defaults to installed Chrome.
Run the product scripts' normal bounded teardown. The 75-second timeout above
belongs only to this small runtime smoke, not full product qualification.

## Repair and evidence

The original WebKit bundle has dangling sys/lib symlinks into the lost
`/tmp/durable-browser-libs`. It was preserved unchanged. A private hardlink copy
of the WebKit bundle lives under
`/workspace/scratch/durable-browser-runtime/browsers/webkit-2359`; only that
copy's stale symlinks were replaced, targeting the private extraction. Chromium
and ffmpeg directories there link to their existing installed bundles.

Nine Debian binary packages were downloaded from the environment's configured
20260828T000000Z trixie snapshot using apt metadata in private workspace paths,
then extracted with `dpkg-deb -x`. No system install, sudo, package database
change, source change, or browser sandbox escalation occurred. Sources and apt
configuration are in `/workspace/scratch/durable-browser-runtime/etc`.
`package-download.log` records versions; `package-sha256.txt` hashes bytes.

Both `webkit-ldd.log` and `webkit-gtk-ldd.log` resolve all linked libraries.
`all-webkit-ldd.json` checks all 15 bundled GTK/WPE ELF files, with no unresolved
linked libraries. Playwright's system-host preflight still reports libGLESv2,
although the private extraction resolves it. Only after those actual ldd checks,
the supported `PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1` override was used.
The warning is retained in `final-smoke.stderr`. This override does not disable
browser sandboxing. The inherited proxy and TLS settings were preserved.

Initial failure is retained in `initial-smoke.json`; the private normal-preflight
failure is retained in `private-smoke.json`. Do not relabel those runs as passes.
