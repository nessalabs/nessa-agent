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
refused, shows *"Nessa couldn't start."* with **Try again** (restarts the app)
and a **Details** disclosure holding the technical reason and a copy button.
No command that needs `HostDependencies` can be reached from that page.

### 2. One narrow repair: shared read on a private file

`nessa-local-storage` gains `repair_shared_read`. It tightens a file or
directory to owner-only when all of these hold, checked on the open descriptor
(`O_NOFOLLOW`, then `fstat`, then `fchmod`):

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

The settings adapter, shared by `settings.json` and `shortcuts.json`, applies
the repair to the config directory and then the file, only after the read was
refused as unsafe, and reads once more. Each repair is written to
`<config root>/local-storage-repairs/` before and after the change: target,
mode before and after, cause `shared_read`, initiator `desktop_host`. If the
intent record cannot be written, nothing is changed and the read stays refused.
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

Every `launchctl` call the registration makes is bounded (30 s). Copying the
bundled runtime is local file I/O and is the one step without a timer. A step
that runs past its bound ends the attempt, and the attempt's settlement is what
moves the projection to `Failed`. The panel keeps no timers and makes no
guesses.

`Failed` keeps the host's technical message, for **Details** only. The panel
and onboarding show *"Nessa couldn't start."* with **Try again**. No failure
reason is inferred from the message's text.

The panel subscribes to the startup projection the way onboarding does, through
one shared monitor, and shows it in the composer notices. The conversation
list's empty state no longer says "gateway".

### 4. The old gateway says why it will not retire

`result.json` gains `refusal` whenever `retired` is `false`:

| `refusal` | The gateway sets it when | The host does |
| --- | --- | --- |
| `data_missing` | its conversation directory no longer exists when it is asked to retire | Stops the old service itself (plan `unload-unretirable-service`), after the history records `RetirementRefusedDataMissing`, then continues |
| `not_confirmed` | anything else: a stop, an agent's cleanup, or the retirement audit could not be confirmed | Fails the attempt; **Try again** asks again |
| absent | the gateway predates this field | Same as `not_confirmed` |

`data_missing` is safe to act on because the evidence the refusal protects is
gone. Stopping the process loses nothing that still exists. The host's own
journal records the stop: intent, plan, completion, observation and outcome,
with the gateway's refusal as the cause.

The history's fact order gains one path. When a managed gateway was running,
the first fact is either `RetirementAcknowledged` or
`RetirementRefusedDataMissing`, and `OldServiceUnloaded` follows either one.

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
- A downgraded host paired with a newer gateway cannot parse `refusal` and waits
  out its 75 s timeout. This is accepted: host and gateway ship together, and
  only a downgrade pairs them this way.
- A new `refusal` value, or a new repair condition, needs its row here first.
