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
just remote test            # the workspace tests, on the sandbox
just remote server          # the gateway there, at 127.0.0.1:7421 here
just remote web             # Vite there, at 127.0.0.1:1420 here
just start-remote           # the desktop app here, everything else there
```

## `just remote <recipe>`

Runs a recipe on this checkout's builder sandbox
([scripts/remote/boat.sh](../../scripts/remote/boat.sh)). What a recipe writes
stays there: `just remote release` builds a Linux package on the sandbox, not an
app here.

Each checkout has its own builder, so worktrees neither queue on one Cargo lock
nor collide on one port. The main checkout's is created on first use (`large`:
8 vCPU, with the account's Boat environment); a worktree's is forked from the
main one's latest snapshot, so it starts with a warm Cargo cache and whatever
was signed in there. Ids are recorded under `~/.config/nessa/boat-builders/`,
and `just worktree remove` deletes the worktree's builder. The first command on
a builder installs mold, cargo-nextest, and just.

Before the recipe runs, the checkout is copied there with `rsync`, leaving out
what git ignores, and then re-copied every second while it runs, so a saved edit
reaches a dev server about a second later. On the sandbox,
[scripts/remote/prepare.sh](../../scripts/remote/prepare.sh) fills Nessa UI from
its pin, installs Linux `node_modules`, and installs the ACP harnesses, each only
when its lockfile changed. `127.0.0.1:1420` and `:7421` are forwarded here when
nothing here holds them already.

Other entry points:

```sh
bash scripts/remote/boat.sh shell     # a shell in this checkout's remote copy
bash scripts/remote/boat.sh proxy     # SOCKS on :1080 to reach any sandbox port
bash scripts/remote/boat.sh stop      # stop the builder now
```

The builder runs on Boat credit while it is up, so it stops itself: each
running `exec`, `shell`, or `sync --watch` holds a lease, and when the last one
ends the builder is stopped. Its disk, and Cargo's cache, survive, and the next
`just remote` resumes it warm. Set `NESSA_BOAT_KEEP=1` to leave it running.

`just test` runs the desktop host's tests without `custom-protocol`, as CI does,
so no frontend build has to come first.

## `just start-remote`

[scripts/remote/start.sh](../../scripts/remote/start.sh) runs the gateway and Vite
on the sandbox ([stack.sh](../../scripts/remote/stack.sh)), forwards `:7421` and
`:1420` (and refuses if either is taken here), and launches a desktop host that
was compiled by CI rather than here. The host is the debug build without
`custom-protocol`, the same one `tauri dev` runs, so it loads its page from Vite:
UI edits reach it without a new host. Quitting the app stops the remote stack.

The gateway uses this Mac's dev credentials from `~/.nessa/dev/auth` when they
exist, and its endpoint record is copied back after every start, because that
record is what the app trusts. **Those credentials then live on the builder, and
in its snapshots, beside whatever the account's Boat environment injects.**

Agents run on the sandbox. Sign them in once on the main checkout's builder,
with `bash scripts/remote/boat.sh shell` and then `claude` or `codex login`;
worktrees' builders are forked from it and inherit the login.

### Where the host binary comes from

[scripts/remote/macos-app.sh](../../scripts/remote/macos-app.sh) snapshots the
working tree, uncommitted and untracked files included, through a throwaway index
— the branch and the real index are untouched. CI
([macos-dev-app.yml](../../.github/workflows/macos-dev-app.yml)) builds whatever
is pushed to `ci/scratch/<login>`, one branch per person replaced on every build.
It never runs on pull requests. It also runs on `main`, only to keep the Rust
cache that scratch builds read warm.

Each build is published with the files Cargo compiled it from, taken from its
dep-info ([host_inputs.py](../../scripts/remote/host_inputs.py)). A downloaded
build is reused while those files, and every manifest, lockfile, build script,
and toolchain file in the tree, are unchanged, so an edit only Vite serves never
waits on CI. [macos-app.test.mjs](../../scripts/remote/macos-app.test.mjs) covers
which edits reuse a build and which do not.

**The repository is public, and so is each snapshot.** Unfinished work pushed
for a build is visible on `ci/scratch/<login>` until the next one replaces it.
The snapshot is a commit with no parent, so unpushed history does not go with it.
