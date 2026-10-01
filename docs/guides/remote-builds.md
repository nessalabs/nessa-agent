# Build off the laptop

A checkout's Cargo output grows past 100 GB of test binaries and incremental
state, and every test round spends the laptop's CPU on `rustc`. These commands
keep the files, and the agents editing them, on the Mac and move compilation
elsewhere (#358):

- **Linux work runs on a Boat sandbox**: tests, the gateway, Vite, and the
  agents the gateway runs.
- **The macOS desktop host is built by GitHub Actions**, from a snapshot of the
  working tree.

```sh
just remote test            # any recipe, on the sandbox
just remote server          # the gateway there; just remote web for Vite
just start-remote           # the desktop app here, everything else there
```

## `just remote <recipe>`

Runs a recipe on the builder sandbox
([scripts/remote/boat.sh](../../scripts/remote/boat.sh)). On first use it creates
the builder (8 vCPU, the account's Boat environment, mold, cargo-nextest) and
records its id in `~/.config/nessa/boat-builder`; a stopped builder is resumed
from its snapshot, so Cargo's cache survives.

Before the recipe runs, the checkout is copied there with `rsync` (honouring
`.gitignore`) and then re-copied every second while it runs, so a saved edit
reaches a dev server within about a second. On the sandbox,
[scripts/remote/prepare.sh](../../scripts/remote/prepare.sh) fills Nessa UI from
its pin, installs Linux `node_modules`, and installs the ACP harnesses, each only
when its lockfile changed. Every checkout and worktree shares one remote
`CARGO_TARGET_DIR`. `127.0.0.1:1420` is forwarded here, so `just remote web`
opens at the usual address.

Other entry points:

```sh
bash scripts/remote/boat.sh shell     # a shell in this checkout's remote copy
bash scripts/remote/boat.sh proxy     # SOCKS on :1080 to reach any sandbox port
bash scripts/remote/boat.sh stop      # snapshot and stop the builder
```

The builder runs on Boat credit while it is up. Stop it when you are done; the
next `just remote` resumes it warm.

## `just start-remote`

[scripts/remote/start.sh](../../scripts/remote/start.sh) runs the gateway and Vite
on the sandbox ([stack.sh](../../scripts/remote/stack.sh)), forwards `:7421` and
`:1420`, and launches a desktop host that was compiled by CI rather than here.
The host is the debug build without `custom-protocol`, the same one `tauri dev`
runs, so it loads its page from Vite: UI edits reach it without a new host.

The gateway uses this Mac's dev credentials from `~/.nessa/dev/auth` when they
exist, and its endpoint record is copied back after every start, because that
record is what the app trusts. Agents run on the sandbox; sign them in there
once with `bash scripts/remote/boat.sh shell`, then `claude` or `codex login`.
Quitting the app stops the remote stack.

### Where the host binary comes from

[scripts/remote/macos-app.sh](../../scripts/remote/macos-app.sh) snapshots the
working tree, uncommitted and untracked files included, through a throwaway index
— the branch and the real index are untouched. CI
([macos-dev-app.yml](../../.github/workflows/macos-dev-app.yml)) builds whatever
is pushed to `ci/scratch/<login>`, one branch per person replaced on every build,
and never runs on pull requests or `main`.

Each build is published with the files it was compiled from, taken from Cargo's
dep-info, plus the manifests and lockfile. A downloaded build is reused while
those files are unchanged in the current tree, so an edit only Vite serves never
waits on CI. A host change rebuilds in about a minute with a warm cache.
[macos-app.test.mjs](../../scripts/remote/macos-app.test.mjs) covers which edits
reuse a build and which do not.

**The repository is public, and so is each snapshot.** Unfinished work pushed
for a build is visible on `ci/scratch/<login>` until the next one replaces it.
