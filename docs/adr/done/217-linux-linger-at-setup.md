# 217. Linux setup offers linger as a choice, and only logind's answer is claimed

## Purpose

On Linux the gateway is a systemd user unit. It runs while the person is signed
in and stops at a full logout. Keeping it running after logout is linger
(`loginctl enable-linger`). For the caller's own uid, logind authorizes
`org.freedesktop.login1.set-self-linger`. From systemd v249 through current
main, that action's defaults are `allow_any`, `allow_inactive`, and
`allow_active` = `yes`, so a stock system does not ask an administrator.
`org.freedesktop.login1.set-user-linger` (`auth_admin_keep`) applies only when
the uid is someone else's. Setup may offer linger, and may say the gateway
keeps running after logout only when a confirming read says so. Registration
does not require linger
([the Linux plan](../../todo/desktop-linux-windows-plan.md#4-linux)).

- **Date:** 2026-10-08
- **Status:** accepted
- **Issue:** [#217](https://github.com/nessalabs/nessa-agent/issues/217)

## Context

`SetUserLinger` returns before anyone should trust it. The reply can be lost
because the app quit, and linger can be turned on or off outside the app. The
method's `Ok` is not the property. A stored "we enabled it" would still be on
screen after logind had been changed back.

The binding constraint is the one in the plan: never enable linger silently,
and never claim logged-out operation unless logind confirms it.

## Decision

Linux setup, after the summon step, asks whether the gateway should keep
running after logout. That question is the only way this process calls
`org.freedesktop.login1.Manager.SetUserLinger` for the current user, with
`enable` true and `interactive` true. `interactive` lets a distro that
overrides the stock policy still ask; the wait is 25 seconds, then the call is
a refusal. macOS, Windows, and a browser have no logind question; setup
finishes from the summon step. Choosing "only while signed in" finishes setup
and does not call. The app does not turn linger off, and it does not write an
audit file: the confirming read is the report.

What the screen shows is `show(observation, call)` below. There is no attempt
kept in the process. The next `status` is a fresh read (`call` is absent).
Quitting during the call is that next read.

An observation is one read. `GetUser` returning `NoSuchUser` is not a linger
bit: the read falls back to `/var/lib/systemd/linger/<user>`, the file logind
itself writes. If that file cannot be named or read, the observation is
`unreadable` and linger is not offered.

| Observation | Meaning |
| --- | --- |
| `enabled` | `Linger` is true, or the linger file is present |
| `disabled` | the read answered and linger is off |
| `unsupported` | the system bus cannot be reached, or logind has no owner |
| `unreadable` | logind answered and linger could not be read |

| Observation | Call | Shown | Claims logged-out operation | Calls `SetUserLinger` |
| --- | --- | --- | --- | --- |
| `enabled` | absent, `succeeded`, `refused`, or `failed` | `enabled` | yes | no |
| `unsupported` | any | `unsupported` | no | no |
| `unreadable` | any | `failed` | no | no |
| `disabled` | absent | `offer` | no | no |
| `disabled` | `refused` | `refused` | no | the call already returned |
| `disabled` | `succeeded` | `failed` | no | the call returned ok; the read is still off |
| `disabled` | `failed` | `failed` | no | the call failed for a reason that is not a refusal |

`not-applicable` is not a row. It is the answer on a host that has no logind
API, and that host does not call `SetUserLinger`.

A lost reply is the absent-call column on the next read:

| Read after the process is gone | Shown | Claim |
| --- | --- | --- |
| `enabled` | `enabled` | yes, because this read says so |
| `disabled` | `offer` | no |
| `unsupported` | `unsupported` | no |
| `unreadable` | `failed` | no |

Linger changing outside the app is the next `status` read. There is no
subscription. Accept calls only when the read just taken says `disabled`.
`status` never calls. A refusal or a failure tells the person they can run
`loginctl enable-linger`.

A call error name that is polkit's cancel or denial, or a D-Bus timeout or
no-reply, is `refused`. Any other call error, including an unknown name, is
`failed`. Unknown names are not shown as a refusal.

## Alternatives considered

- **Enable linger at install, with no question.** Rejected. It is an
  account-wide policy, and the plan forbids enabling it silently.
- **Tell the person an administrator must approve.** Rejected. For this uid,
  stock systemd does not ask. The copy would describe a prompt that does not
  appear.
- **Treat the method's `Ok` as success.** Rejected. `Ok` while linger is still
  off would claim logged-out operation logind does not confirm. That screen is
  `failed`, not `refused`: the call was not refused.
- **Remember the call and restore it next launch.** Rejected. The next process
  has only the read.
- **Hold the window thread for the call.** Rejected. The command runs on the
  blocking pool.
- **Turn linger off from the app.** Not this decision. The person can run
  `loginctl disable-linger`.

## Consequences

Linux setup gains one step. The other hosts do not. A person who declines, or
whose call is refused or fails, finishes setup with the gateway still tied to
the login session, and can turn linger on with `loginctl enable-linger`. A
machine with no logind is told that, and is not offered the call.

The screen can be stale after the person leaves the step: nothing in the panel
re-reads linger.
