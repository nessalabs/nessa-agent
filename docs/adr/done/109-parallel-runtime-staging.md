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
6,337-file tree on a GitHub macOS runner. Per-file `sync_all` was about half
of the earlier 8.8 s. Parallel sync slowed the single copying thread (clone
886 ms to 2,686 ms). A drag-installed app has no installer hook. The
fingerprint directory is created only by exclusive rename. A later validation
failure keeps that name.

Saurav then ran `measure_runtime_staging_phases` on a signed MacBook Air
(arm64, macOS 26.6, release, 10 workers, cache purge succeeded) at the head
that copied on a pool of that size. Wall was 11,230.554 ms. Accounted time
was 114,573.426 ms because the phase timers overlap across workers. The
phases, in milliseconds:

| Phase | Accounted | Per file |
| --- | ---: | ---: |
| clone | 42,352.507 | 6.68 |
| xattr | 29,408.691 | 4.64 |
| permissions | 19,993.982 | 3.15 |
| open | 11,558.242 | 1.82 |
| file_sync (span) | 10,363.724 | |
| file_sync_work (sum) | 51,109.59 | 8.07 |
| digest | 807.479 | |
| dir_sync | 21.596 | |
| parent_sync | 11.417 | |
| private_walk | 55.1 | |
| manifest | 0.44 | |
| publish | 0.246 | |
| byte_copy | 0 | |

The source was 393,574,780 bytes, 6,337 files, all cloned, 685 directories,
8 symlinks. First exec of the staged node was 1,228 ms; the same node still
quarantined was 2,522 ms; the second exec was 19.9 ms. A 128-file microbench
on that machine: `sync_all` 15.754 ms, then `F_FULLFSYNC` 1.203 ms and
`fsync` 1.226 ms on files that had already been synced.

`byte_copy` is zero, so this is not a bandwidth problem. Each cloned file
still pays `flistxattr` twice, one `fremovexattr` per name (quarantine is on
every file of a drag-installed app; `com.apple.provenance` returns `EPERM` and
is left in place), then `flistxattr` twice more to prove the file is clean.
That is the xattr phase. The permissions phase is `fchmod` to `0600` plus the
source execute bits, because the clone kept the bundle mode. The open phase
is the source and the destination. Ten threads doing those metadata calls on
one APFS volume take the same lock, so the wall time is the copy pool
(about 11.2 s) and the sync span (10.4 s) hides inside it. The summed
`sync_all` work is about 8.1 ms per file, roughly 65 times the uncontended
microbench (0.12 ms). The 1.2 ms `F_FULLFSYNC` and `fsync` figures are a
second flush of files that were already synced, not a cheaper primitive.

The 6,961 ms baseline is a different machine. It does not say whether this
Mac got faster.

## Decision

Survey the tree, send the zero progress report, then `clonefile` the source
directory onto `.staging-<id>` in one call. The clone is then compared with
that survey without following links. A path whose type is not the surveyed
one, including a directory or file replaced by a symlink and a symlink the
survey did not record, refuses the attempt before any mode, attribute, or
sync call. One thread then sets every directory to `0700`, and for each file
opens it, seals the mode and extended attributes, and runs `sync_all`.
Progress counts a file only after that `sync_all` returns. The digest runs
after that loop: the fingerprint includes
execute bits, and on one thread a hash started earlier would read a file
that is not sealed yet. Directory sync, the publication checks, and one
`renamex_np` follow, unchanged. The per-file fallback still overlaps the
digest with `sync_all`, and only after every seal has returned.

`ENOTSUP` and `EXDEV` leave no tree (`clonefile` is all-or-nothing on
failure) and the per-file copy runs on one thread, not on a pool. Any other
error fails the attempt. An already valid generation is not copied.

`copyfile` with `COPYFILE_RECURSIVE` is not this path. It still clonefiles
each file and creates each directory, and current `copyfile` rejects
`COPYFILE_RECURSIVE` together with `COPYFILE_CLONE_FORCE`.

Per-file `sync_all` stays. Skipping it lets a byte-copy digest match cached
bytes, the rename succeed, and the next launch hit Preserved and refuse to
replace the directory. A directory `fsync` does not flush child inode
metadata on APFS (`dir_sync` for the whole tree was 21 ms). One volume
`sync` is the wrong scope. The uncontended cost of 6,337 `sync_all` calls is
about 0.8 s; the 51 s sum was contention.

The copy still starts only after the empty-cleanup `stage-runtime` plan is
accepted, on the thread that holds the label lock. Ready is published before
that thread runs, so the copy adds nothing to time-until-setup-is-usable.
The bar can appear at the zero report, which is sent before `clonefile`.
Quarantine is still stripped: a quarantined first exec was 2,522 ms against
1,228 ms staged.

The injected per-file pool remains for tests that supply a clone and assert
overlap. Production does not use it.

### States

| State | Entered when | What holds |
| --- | --- | --- |
| Unpublished | No fingerprint directory and no temp directory for this attempt | The gateway runs the bundle. Ready may already be set, before `.staging-<id>` exists. |
| Copying | This attempt's temp directory exists and rename has not run | The fingerprint name is absent. The digest thread and every file sync finish before the temp directory is removed. |
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
| The plan is known and no file has synced | One report, zero files completed. This is before `clonefile`. |
| `sync_all` returned, the percent is not higher than the last report, and files remain | Nothing. The count includes the file. |
| `sync_all` returned and the percent is higher, or this is the last file | One report. The callback returns before the next file is counted. |
| The last file's report has been sent | Later reports are dropped. |
| A subscriber starts after a report was emitted | It reads the latest retained report. The host writes that report before emitting it. |
| A report arrives after subscribe, including while the read is in flight | The event is kept. The read is not applied over it. |
| No report has been retained | The read is empty. The bar stays absent. |
| Subscribing fails | The retained report is still read. A failed read leaves the bar absent. |

## Alternatives considered

- **Hardlinks.** They lose. Mode and extended attributes are on the inode, so a
  private file cannot share one with the bundle. Published files must have
  `nlink == 1`.
- **Skip per-file `sync_all`, or sync only the directories at publish.** It
  loses. The digest can match cached bytes, the rename can succeed, and the
  next launch then hits Preserved and will not replace the directory. Directory
  `fsync` does not persist the new inode metadata. The microbench's 1.2 ms
  figures are a second flush, not a reason to drop the first.
- **One `sync` of the volume at publish.** It loses. That call is not scoped
  to this tree and can stall the machine.
- **Start the copy before the plan is accepted.** It loses. Pruning removes
  `.staging-` directories while it holds the label lock.
- **Ten copy workers, and a sync pool of the same size.** It loses on this
  Mac. Accounted clone, xattr, permissions, open, and `sync_all` all ran at
  once and the wall time was 11.2 s. The file walk on one thread is the
  fallback and the comparison's old path, not a pool.
- **`copyfile(COPYFILE_RECURSIVE)`.** It loses as the fast path. It walks the
  tree and clonefiles each file. `COPYFILE_CLONE_FORCE` combined with
  `COPYFILE_RECURSIVE` is `EINVAL` in current `copyfile`.
- **Put progress on `GatewayStartup`.** It loses. That projection is what
  disables the agent list while startup is `starting`.

## Consequences

Setup can answer `GET /onboarding/agents` while the copy runs. The bar is how
many files have finished `sync_all`, not a claim that the generation is
published. On the shipping path the descriptors are one destination file at
a time, plus the digest's reads. The per-file fallback uses one copy thread
and one sync thread.

This Linux workspace cannot run the copy. The wall time below is not a
measurement of the new path. On the signed Mac, after `sudo -v`:

```text
sudo -v
NESSA_STAGING_PURGE=required cargo test -p nessa-app --release compare_runtime_staging_strategies -- --ignored --nocapture --test-threads=1
```

That prints `copy_wall_ms`, `first_progress_ms`, and
`setup_usable_added_ms` for the old one-thread per-file copy and for the
tree clone, side by side. `setup_usable_added_ms` is 0 on both: Ready is
already published. `first_progress_ms` is when the bar can appear. Signed-Mac
timing stays #562.
