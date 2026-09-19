# 0005. On-disk client data is namespaced; only `prod` uses the bare path

## Purpose

Keep local settings, credentials, and caches separated by stage and instance.
Development worktrees and test runs can coexist without overwriting everyday
production data.

- **Date:** 2026-09-03
- **Status:** accepted

## Context

The same machine will run many Nessa processes at once: day-to-day **dev**,
**alpha** / **ci** builds, **prod**, extra named environments we invent later,
and **several git worktrees**. Today the host writes `settings.json` (and soon
other files) into a single app config directory. Those processes can read or
overwrite each other’s preferences, window geometry, and caches.

We already pass a stage on the wire and in env. That value must partition
**local** data — and it must not be a closed enum of three non-prod names. Any
propagatable stage string should get its own tree so we can run as many
versions as we need.

## Decision

### Roots

| Stage | Root under the app config directory |
| --- | --- |
| exactly `prod` (case-insensitive) | bare app config dir — **no** namespace segment |
| **any other non-empty stage string** | `<app config dir>/<namespace>/` |

Only **`prod`** is special. `dev`, `alpha`, `ci`, `staging`, `feat-foo`, etc.
are all ordinary namespaced stages — conventions, not an exclusive allow-list
for paths.

### Namespace

Derived from process env (not from the remote URL mid-session):

1. **Default:** the stage string itself (filesystem-sanitized).
2. **Worktree / sandbox isolation:** when `NESSA_INSTANCE` is set, the namespace
   is `<stage>-<instance>` (both segments sanitized). Two checkouts with the
   same stage and different instances do not share files; omit instance to share
   the stage default on purpose.

`NESSA_STAGE` unset: `dev` in debug builds, `prod` in release.

`settings.json`, `shortcuts.json` ([0004](0004-server-owned-keybindings.md)),
and any later local stores use that root.

**Protocol / env:** stage remains a propagatable string end-to-end. Known
values (`dev`, `alpha`, `ci`, `prod`) may keep documented **policy** (e.g. who
may omit a token); path isolation must not require membership in that list.
Adding a new stage name must not need a code change to the path helper.

New on-disk stores follow this by default.

### The loopback port is stage-scoped too, but by a closed table

Disk is cheap: any stage string can have a directory nobody else wants. A TCP
port is not. Two stages cannot share 7420, and there is no way to derive a
distinct free port from an arbitrary stage name.

So the port comes from a named table,
`protocol/defaults/gateway-ports.json`, keyed by the stages this codebase
actually runs — deliberately the "policy table for known stages" this ADR allows,
not a path rule. `prod` keeps 7420, the port a packaged install holds through its
launchd background service; `dev` takes 7421 so a checkout and an installed app
can both run. A stage the table does not name has no default port and must set
`NESSA_PORT`, which overrides the table everywhere and in every stage.

The table is one file read by `nessa-server`, the desktop host, the frontend and
the Vite dev proxy, so the answer cannot differ between them.

## Alternatives considered

- **Closed enum for paths (`dev` \| `alpha` \| `ci` only).** Blocks arbitrary
  sandboxes and future named channels without a code change. Rejected for
  storage; policy tables may still name known stages.
- **Namespace `prod` too.** Breaks “one production profile.” Rejected.
- **Separate OS app / bundle ids per stage.** Strong isolation, heavy
  packaging. Rejected for now.
- **Suffix filenames** instead of directories. Easy to miss a file. Rejected.

## Consequences

Easier: any number of named Nessa versions on one machine; worktrees opt into
isolation via instance; deleting a namespace directory resets that sandbox;
prod stays untouched.

Harder: path helpers take an open stage string (+ optional instance); wire and
env validation must allow unknown stages while still applying known-stage
**auth** policy; migrating today’s flat `settings.json` into prod’s bare path
needs a note in the implementing PR.

Watch for: treating “not in {dev,alpha,ci,prod}” as invalid for disk; writing
a non-`prod` build into the bare config dir; resolving namespace from the
server URL.
