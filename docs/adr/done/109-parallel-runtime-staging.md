# 109. Stage the bundled runtime in parallel, and show the copy

## Purpose

First launch copies the bundled runtime into a private generation. That copy
must not sit in front of the gateway, and setup must show it moving. #853
already runs an unpublished generation from the bundle and returns surface
readiness before the copy finishes. This record decides how the copy runs,
which cheaper copies are allowed, and how progress stays off that readiness.

- **Date:** 2026-10-20
- **Status:** accepted
- **Issue:** [#109](https://github.com/nessalabs/nessa-agent/issues/109)

## Context

Release measurement #853 (run 38485815250) took about 7.0 s wall for a 390 MB,
6,337-file tree. Per-file `sync_all` was about half of the earlier 8.8 s.
Parallel sync slowed the single copying thread (clone 886 ms to 2,686 ms). A
drag-installed app has no installer hook. The fingerprint directory is created
only by exclusive rename. A later validation failure keeps that name.

## Decision

Count the tree, create directories parent-before-child, then copy files on a
pool sized like the sync pool. Each worker clones with
`copyfile(COPYFILE_CLONE_FORCE)`, or byte-copies on `ENOTSUP` or `EXDEV`, seals
the mode and extended attributes, and queues `sync_all`. The attempt owns the
temp directory. After every file `sync_all`, every staged directory is synced,
then the digest is checked, then one `renamex_np`. The digest overlaps the
remaining syncs. An already valid generation is not copied.

Progress counts a file only after that file's `sync_all` returns. It is a host
event, not `GatewayStartup`. Dropped events leave the figure stale. A report
runs to completion before the next file is counted, so a slower report cannot
move the percent backwards or follow the final file. Setup draws it and does
not disable agents because of it. The copy still starts only after the
empty-cleanup `stage-runtime` plan is accepted, on the thread that holds the
label lock.

### States

| State | Entered when | What holds |
| --- | --- | --- |
| Unpublished | No fingerprint directory and no temp directory for this attempt | The gateway runs the bundle. Ready may already be set, before `.staging-<id>` exists. |
| Copying | This attempt's temp directory exists and rename has not run | The fingerprint name is absent. Pools join before the temp directory is removed. |
| Published | `renamex_np` succeeded | The fingerprint name exists. A parent `sync_directory` error afterwards does not remove it. |
| Attempt failed | An error returned before rename | The fingerprint name is absent. `TemporaryRuntime` removes only this temp directory. |
| Preserved | The fingerprint directory exists and validation fails | The name stays. Nothing is copied. This is not Attempt failed. |
| Reused | The fingerprint directory validates | No copy and no progress. |
| Orphan temp | The process died while `.staging-<id>` existed | A later registration's pruning, holding the label lock, removes it. |

Ready may be observed in Unpublished and in Copying. A crash is Orphan temp,
not Attempt failed. Rename is the only writer of the fingerprint name.

### Progress reports

Completed bytes are the sizes from the walk. A later length is not counted.

| Order | What is sent |
| --- | --- |
| The plan is known and no file has synced | One report, zero files completed. |
| `sync_all` returned, the percent is not higher than the last report, and files remain | Nothing. The count includes the file. |
| `sync_all` returned and the percent is higher, or this is the last file | One report. The callback returns before the next file is counted. |
| The last file's report has been sent | Later reports are dropped. |

## Alternatives considered

- **Hardlinks.** They lose. Mode and extended attributes are on the inode, so a
  private file cannot share one with the bundle. Published files must have
  `nlink == 1`.
- **Skip per-file `sync_all`.** It loses. The digest can match cached bytes, the
  rename can succeed, and the next launch then hits Preserved and will not
  replace the directory.
- **Start the copy before the plan is accepted.** It loses. Pruning removes
  `.staging-` directories while it holds the label lock.
- **Keep the file walk on one thread.** It loses. That thread is the byte path,
  and the sync pool already slowed it.
- **Put progress on `GatewayStartup`.** It loses. That projection is what
  disables the agent list while startup is `starting`.

## Consequences

Setup can answer `GET /onboarding/agents` while the copy runs. The bar is how
many files have finished `sync_all`, not a claim that the generation is
published. Extra descriptors are the sync queue (`workers * 2`), one file each
sync worker has popped, and one source and one destination per copy worker.
Signed-Mac timing stays #562: rerun
`measure_runtime_staging_phases` as in
[`.github/workflows/stage-runtime-phases.yml`](../../../.github/workflows/stage-runtime-phases.yml)
and compare `phases_ms.wall` with 6,961 ms from run 38485815250.
