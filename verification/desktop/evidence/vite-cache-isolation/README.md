# Verification cache isolation (#1035)

The maintained regression is `../../scripts/lib/server-cache.test.mjs`.
It uses real Vite and React in disposable fixture storage, keeping a canonical
development server alive while another optimizer runs. The shared-cache control
overwrites its on-disk JSX runtime with a production runtime whose `jsxDEV` is
undefined. Opt-in caches preserve the canonical bytes and callable development
runtime, including while a separate production optimizer runs. Cleanup tests
delay Vite shutdown and fail configuration deliberately.

Local focused result: 10 launcher tests pass. Four rule-removal probes are
caught in [probes.json](probes.json); originals restored, focused tests rerun.
This cloud task reused installed Vite 7.3.6 and React 19.3.0 without installing;
the host lockfile pins React 19.2.8, so CI qualification is still required.
The broader helper suite has 366 passes and one missing-`tsx` capability failure.
No complete local frontend, browser or native pass is claimed.

The original Mac canonical `jsxDEV` browser failure remains unexplained.
The separate Mac probe proved disk contamination but did not reproduce the
browser exception: both imports succeeded and its reproduction assertion failed.
This regression proves the cache hazard and isolation contract, not attribution
of that original failure. After integration, preserve runtime/network evidence
from controlled concurrent capture/canonical checks and a standalone canonical
run against the final combined source.
