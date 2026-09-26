# 221. Nessa handles a startup refusal itself, or says so in plain words

## Purpose

Nessa is for people who are not technical. When startup meets something it must
refuse, such as a file others can read or an old gateway that will not step
aside, Nessa either fixes it itself and records what it did, or shows one plain
sentence with **Try again**. It never crashes to enforce a rule and never shows a
spinner for something that has already failed. This record settles the three
decisions [#221](https://github.com/nessalabs/nessa-agent/issues/221) left open:
the one exception to "unsafe files are never repaired", why an old gateway can
refuse to retire, and what the host does about each reason.

- **Date:** 2026-09-26
- **Status:** accepted
- **Issue:** [#221](https://github.com/nessalabs/nessa-agent/issues/221)

## Context

On 2026-09-26 one installed Mac hit three startup failures in a row (#221 has
the full account):

1. `settings.json` was `0644`. `nessa-local-storage` refuses it, `assemble`
   returned the error through Tauri's `setup` hook, Tauri panicked inside
   `applicationDidFinishLaunching`, and the process aborted.
2. The old gateway refused to retire because its conversation directory had
   been moved to the Trash, so it could not record that it stopped its agents.
   `result.json` said why only as a debug string. The host failed startup, and
   the panel, which never reads the startup state, said "Connecting to the
   gateway…" forever.
3. The macOS journal reader refused the records the macOS writer had just
   written (fixed in the first commit of this change).

Each refusal was correct. What was wrong is what happened next. The binding
constraint is that the person cannot be asked to judge Nessa's internals, so
every refusal must end in one of three places: Nessa handles it, Nessa asks
about the person's own work, or Nessa shows a plain failure with a way to try
again.

## Decision

### 1. Setup always opens a window

`HostDependencies::assemble` and the plugin registrations in `setup` return a
typed `StartupRefusal` instead of an error. `setup` then manages
`HostStartup::Refused`, shows the main window, and does nothing else: no
gateway, no tray, no shortcuts. The page asks `host_startup` first and, when
refused, shows *"Nessa couldn't start."* with **Try again** (restarts the app),
**Quit**, and a **Details** disclosure holding the technical reason and a copy
button. The panel joins the taskbar, since there is no tray. The commands that
need `HostDependencies` stay registered; the refused page never mounts anything
that calls them, and if one were called it would answer that its state is not
there rather than panic.

### 2. One narrow repair: shared read on a private file

`nessa-local-storage` gains `find_shared_read`. It opens the object without
following a final symlink and checks it with `fstat`; a repairable one comes
back held open, and `SharedReadCandidate::tighten` changes that same descriptor
with `fchmod`, so nothing can be swapped in between the check, the record, and
the change:

| Found | Action |
| --- | --- |
| Already private (`mode & 0o077 == 0`), owned by us | Nothing to repair |
| Owned by us, no group or other **write** bit, others can read or search; a regular file with one link, or a directory | Record intent, `fchmod` to `mode & 0o700`, record outcome |
| Group or other can **write** | Refused: the contents may not be ours |
| Owned by another user, a symlink, a hard-linked file, or another file type | Refused |

Nessa has no OS identity of its own; it runs as the person. "Owned by the
current user and writable only by them" is therefore the strongest fact the OS
can give, and the existing check already trusts that user. The repair closes
confidentiality and never widens who is trusted. A "written by Nessa only" rule
would need a signature or recorded identity that any process running as the
person could forge.

The settings adapter, shared by `settings.json` and `shortcuts.json`, repairs
the config directory before each read when it is shared, and repairs the file
only after its read was refused as unsafe, then reads once more. Reading never
depended on the directory, so a directory that cannot be repaired is left as it
was and the read goes ahead as before. Each repair is written to
`<config root>/local-storage-repairs/` before and after the change: target,
mode before and after, cause `shared_read`, initiator `desktop_host`. If the
intent record cannot be written, nothing is changed and the read stays refused.
If the change is made but its outcome cannot be recorded, the repair is not
confirmed and this read stays refused; the next read finds the file private.
Repairs in one process run one at a time, so two reads never record the same
change. The records go through an audit port, as the credential-save audit's
do, and carry no clock of their own, as that audit's records do not.
Anything else unsafe stays refused, and for the gateway's section of
`settings.json` that means the "couldn't start" window, because guessing the
port, instance or data root could start a second gateway.

This is the only exception to "existing unsafe files are rejected, never
silently repaired". The crate documentation says so where it states the rule.

### 3. The startup projection names its step

`Starting` carries a step. The adapters report it through
`GatewayReconciliationProgress::step_started`:

| Step | Sentence the panel shows | macOS | Linux | Longest wait inside it |
| --- | --- | --- | --- | --- |
| `preparing` | Getting ready… | lock, status, staging | staging, unit files | 120 s lock; `launchctl` calls bounded |
| `replacing` | Finishing the last update… | retirement, unload | retirement, stop | 75 s acknowledgement |
| `launching` | Starting… | bootstrap, readiness | start, readiness | 75 s (macOS), 45 s (Linux) |

The waits are each step's own deadline; a `launchctl` call inside one can add
up to its own bound. Every `launchctl` call the registration makes is bounded
(30 s). A `bootstrap` ended at that bound is recorded as indeterminate, not
refused, because launchd may already have accepted it. Copying the
bundled runtime is local file I/O and is the one step without a timer. A step
that runs past its bound ends the attempt, and the attempt's settlement is what
moves the projection to `Failed`. The panel keeps no timers and makes no
guesses.

`Failed` keeps the host's technical message, for **Details** only. The panel
and onboarding show *"Nessa couldn't start."* with **Try again**. No failure
reason is inferred from the message's text.

The panel subscribes to the startup projection the way onboarding does, through
one shared monitor, and shows it in the composer's connection notice, in place
of the session's own until startup is ready. A session that gave up while the
gateway was starting is retried once when startup becomes ready. The conversation
list's empty state no longer says "gateway".

### 4. The old gateway says why it will not retire

`result.json` gains `refusal` whenever `retired` is `false`:

| `refusal` | The gateway sets it when | The host does |
| --- | --- | --- |
| `data_missing` | every agent's stop was confirmed and only its record failed (`AuditFailure` alone, for every owner), and its conversation directory no longer exists | Stops the old service itself (plan `unload-unretirable-service`), after the history records `RetirementRefusedDataMissing`, then continues |
| `not_confirmed` | anything else: a stop that did not finish or was not confirmed, an admission that could not drain, or the retirement audit failing | Fails the attempt; **Try again** asks again |
| absent | the gateway predates this field | Same as `not_confirmed` |

`data_missing` is safe to act on because both halves hold: nothing the gateway
started is still running, and the evidence it could not record has nowhere left
to go. A missing directory alone is not enough; a stop that did not finish keeps
the refusal `not_confirmed` whatever the disk says. The host syncs the result
before acting on it, as it does before acting on a success, and its own journal
records the stop: the failed retirement step, whose text names the refusal, the
plan `unload-unretirable-service`, its completion and observation, and the
outcome. A retired result carries no `refusal` key at all, so it is exactly what
earlier gateways and hosts wrote and read.

The history's fact order gains one path. When a managed gateway was running,
the first fact is either `RetirementAcknowledged` or
`RetirementRefusedDataMissing`, and `OldServiceUnloaded` follows either one.

On Linux the host reads `refusal` and, for now, treats every refusal as
`not_confirmed`, as it did before this record. Acting on `data_missing` there
means changing the systemd retirement and its restart recovery, which is
tracked as its own follow-up rather than done here without a Linux build to
prove it.

The gateway that refused on 2026-09-26 predates `refusal`, so a host with this
change still reads it as `not_confirmed`. The names clear the refusal for every
gateway built from this change onward.

A gateway whose data is missing at retirement reports it. Nothing else changes
about a running gateway: it does not stop itself or report itself unhealthy,
because the retirement request is the only moment the host acts on the answer.

Retirement never refuses because a task is running. It stops running agents,
as it did before this change. Asking before an update interrupts someone's work
is a separate decision, not taken here.

## Alternatives considered

- **Keep crashing on a refused setup, but log better.** A person never sees the
  log. Rejected.
- **Show the file path and a `chmod` command.** It is correct, and useless to
  the people Nessa is for. Rejected.
- **Repair any unsafe file owned by the user.** A group- or world-writable file
  may hold someone else's contents. Rejected; only shared *read* is repaired.
- **Let the panel time out on its own.** A second owner of "how long is too
  long" (gate 13), and it cannot tell a slow step from a stuck one. Rejected.
- **Parse the old `cleanupError` string.** Gate 1. It also fails for every
  gateway that words its errors differently. Rejected; old gateways get
  `not_confirmed`.
- **Ask the person whether to stop an unretirable gateway.** They cannot judge
  it, and for `data_missing` there is nothing to protect. Rejected.
- **Have a gateway with missing data stop itself.** It would also stop a gateway
  whose data was restored a moment later, with nobody asking. Rejected.

## Consequences

- A refused setup is a window, not a crash report, and it carries the reason for
  whoever helps the person.
- The panel says what startup is doing and when it has failed.
- One old-gateway failure, the one that happened, clears itself. Every other
  refusal is still a plain failure with **Try again**. If people get stuck on
  `not_confirmed`, that is the signal to split it further.
- A downgraded host paired with a newer gateway that *refused* cannot parse
  `refusal` and waits out its 75 s timeout; a downgraded gateway reading such a
  stored result refuses it as evidence. Retired results are unchanged, so an
  ordinary upgrade or downgrade is unaffected. This is accepted: host and
  gateway ship together, and only a downgrade after a refusal pairs them this
  way.
- A new `refusal` value, or a new repair condition, needs its row here first.
