# 217. Linux setup offers linger as a choice, and only logind's answer is claimed

## Purpose

On Linux the gateway is a systemd user unit. It runs while the person is signed
in and stops at a full logout. Keeping it running after logout is linger
(`loginctl enable-linger`), an account-wide policy behind the polkit action
`org.freedesktop.login1.set-user-linger`. Setup may offer that, and may say the
gateway keeps running after logout only when logind's own `Linger` property
says so. Registration does not require linger
([the Linux plan](../../todo/desktop-linux-windows-plan.md#4-linux)).

- **Date:** 2026-10-08
- **Status:** accepted
- **Issue:** [#217](https://github.com/nessalabs/nessa-agent/issues/217)

## Context

The prompt can be cancelled, time out, or succeed with the reply lost because
the app quit while it was up. Linger can also be turned on or off outside the
app, before the question, during it, or after. The method's `Ok` is not the
property. A stored "we enabled it" would still be on screen after logind had
been changed back, and a stored "they refused" would hide a linger flag that
was already on.

The binding constraint is the one in the plan: never enable linger silently,
and never claim logged-out operation unless logind confirms it.

## Decision

Linux setup, after the summon step, asks whether the gateway should keep
running after logout. That question is the only way this process calls
`org.freedesktop.login1.Manager.SetUserLinger` for the current user, with
`enable` true and `interactive` true, which is what raises the polkit prompt.
macOS, Windows, and a browser have no logind question; setup finishes from the
summon step as it does today. The app does not turn linger off.

What the screen shows is `show(observation, attempt)` below. The attempt lives
in this process only. A new process starts at `not-chosen` and reads logind
again. Quitting mid-prompt is that new process: the reply is not kept, and the
next read is the whole report.

An observation is one read of this user's `org.freedesktop.login1.User.Linger`,
or the fact that the read could not be made:

| Observation | Meaning |
| --- | --- |
| `enabled` | logind answered and `Linger` is true |
| `disabled` | logind answered and `Linger` is false |
| `unsupported` | the system bus cannot be reached, or logind has no owner |
| `unreadable` | logind answered and `Linger` could not be read |

An attempt is what this process has done about the explicit choice:

| Attempt | Meaning |
| --- | --- |
| `not-chosen` | no choice yet, including a process that replaced one that quit mid-prompt |
| `declined` | the person chose not to enable; no call |
| `in-flight` | `SetUserLinger` has been sent and has not returned |
| `succeeded` | the method returned without an error; not confirmation |
| `cancelled` | the polkit prompt was cancelled or dismissed |
| `not-authorized` | polkit did not authorize the change |
| `timed-out` | the wait ended with no reply |
| `unavailable` | the call failed because logind was not there to answer it |

`enabled` and `unsupported` and `unreadable` decide the screen on their own.
The attempt matters only while logind says linger is off.

| Observation | Attempt | Shown | Claims logged-out operation | Calls `SetUserLinger` |
| --- | --- | --- | --- | --- |
| `enabled` | any, including `not-chosen`, `declined`, `in-flight`, `succeeded`, `cancelled`, `not-authorized`, `timed-out`, `unavailable` | `enabled` | yes | no |
| `unsupported` | any | `unsupported` | no | no |
| `unreadable` | any | `unconfirmed` | no | no |
| `disabled` | `not-chosen` | `offer` | no | no |
| `disabled` | `declined` | `declined` | no | no |
| `disabled` | `in-flight` | `waiting` | no | the one call already sent; a second accept does not send another |
| `disabled` | `succeeded` | `refused` | no | no |
| `disabled` | `cancelled` | `refused` | no | no |
| `disabled` | `not-authorized` | `refused` | no | no |
| `disabled` | `timed-out` | `refused` | no | no |
| `disabled` | `unavailable` | `refused` | no | no |

`not-applicable` is not a row of this table. It is the answer on a host that
has no logind API, and that host does not call `SetUserLinger`.

A lost reply is the `not-chosen` column on the next process, against a fresh
read:

| Read after the process is gone | Shown | Claim |
| --- | --- | --- |
| `enabled` | `enabled` | yes, because this read says so |
| `disabled` | `offer` | no; a refusal that was not observed is not shown |
| `unsupported` | `unsupported` | no |
| `unreadable` | `unconfirmed` | no |

Linger changing outside the app is a new read with the attempt left as it is.
The next `status` replaces the screen. There is no subscription: setup reads
when the step opens, and again inside accept and decline.

| Was showing | New read | Attempt still | Now shows |
| --- | --- | --- | --- |
| `offer` | `enabled` | `not-chosen` | `enabled`; accept does not call |
| `offer` | `unsupported` | `not-chosen` | `unsupported`; accept does not call |
| `enabled` (already on; this process did not call) | `disabled` | `not-chosen` | `offer`; the claim is withdrawn |
| `enabled` after a call | `disabled` | `succeeded` | `refused`; the claim is withdrawn |
| `declined` | `enabled` | `declined` | `enabled` |
| `refused` | `enabled` | `cancelled`, `not-authorized`, `timed-out`, or `succeeded` | `enabled` |
| `unsupported` | `disabled` | `not-chosen` | `offer` |
| `waiting` | `enabled` | `in-flight` until the reply is classed | `enabled` as soon as the read says so |

Accept calls only when the read just taken says `disabled` and no call is
`in-flight`. An intent record (user, before, cause `enable`, initiator
`setup`) is written before that call. If it cannot be written, the call is not
made. After the call returns or times out, linger is read again and an outcome
record (the same user, before, the call's cause, the confirming observation,
initiator `setup`) is written. A failed outcome record does not hide the
confirming read: the screen follows the table and says the record failed.
Decline writes one record and does not call. A decline while a call is
`in-flight` is ignored. `status` never calls and never writes.

The interactive call waits long enough for an administrator to answer the
polkit prompt (three minutes). When that wait ends, the confirming read is the
report, including the case where the change landed and the reply did not.

## Alternatives considered

- **Enable linger at install, with no question.** Rejected. It is an
  account-wide policy, polkit asks an administrator, and the plan forbids
  enabling it silently.
- **Treat the method's `Ok` as success.** Rejected. The reply can be lost, and
  `Ok` while `Linger` is still false would claim logged-out operation logind
  does not confirm.
- **Remember the prompt and restore it next launch.** Rejected. A remembered
  success claims logged-out operation after quit, and a remembered refusal
  hides a flag someone else turned on. The next process has only the read.
- **Watch `PropertiesChanged` for the whole step.** Rejected. Each `status`
  read already replaces the claim. A subscription adds orderings the offer
  does not need.
- **Turn linger off from the app.** Not this decision. The person can run
  `loginctl disable-linger`.

## Consequences

Linux setup gains one step. The other hosts do not. A person who declines, or
whose prompt is refused, finishes setup with the gateway still tied to the
login session. A machine with no logind is told that, and is not prompted.

The screen can be stale after the person leaves the step: nothing in the panel
re-reads linger. A change made outside the app after setup is not shown until
something calls `status` again. The polkit dialog itself is the desktop's
agent; this record does not draw it.
