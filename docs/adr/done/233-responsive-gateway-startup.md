# 233. Keep the gateway separate and make agent startup responsive

## Purpose

Nessa should respond promptly when someone sends a message. We will keep its
local server separate from the chat window, give it app-level scheduling on
macOS, and write simple timing logs to help explain future waits.

- **Date:** 2026-09-27
- **Status:** accepted
- **Issue:** [#233](https://github.com/nessalabs/nessa-agent/issues/233)
- **Evidence:** [startup investigation](../../reviews/startup-latency.md)

## Context

The installed macOS gateway used the Background process setting. Its Claude
processes inherited those restrictions. Four warm tests per setting found
conversation creation at 0.86–3.36 seconds under Background and 0.33–0.60 seconds
under Interactive. These small samples support a change, but do not explain the
original 45-second failure or predict worst-case startup time.

A separate service and low scheduling priority are different choices. The service
needs to handle incoming and outgoing requests even when its window is hidden.
The owner approved Interactive scheduling and local timing logs, and explicitly
excluded UI changes from this work.

## Decision

Use `ProcessType=Interactive` for the managed macOS gateway. Keep its separate
process, normal registration/update path, and existing restart and shutdown rules.
Log process startup, startup exchanges, protocol readiness, prompt dispatch, and
first answer text through the existing tracing subscriber. Use the existing
injected clock for durations. Keep payloads out of these timing lines.

The first-response log measures from prompt dispatch to the first accepted,
nonempty answer chunk. It excludes queueing and startup and is not screen-render
time. There is no metrics backend or external exporter. Durable audit remains
separate; losing a diagnostic line does not change execution decisions.

### Timing cases

This table describes diagnostic bookkeeping only, not new execution states.
The tests live in the SDK's `tests/infrastructure/acp/executions/worker/timing.rs`.

| Event | Timing behavior | Regression evidence |
| --- | --- | --- |
| A prompt begins dispatch | Start its response timer | First-response test calls the real dispatch path |
| Thought, empty answer, or rejected session identity | Keep waiting for answer text | First-response test covers all three |
| First accepted nonempty answer | Log duration and consume the timer | First-response test asserts 50 ms and exact IDs |
| More answer text | No repeated first-response metric | First-response test supplies a second chunk |
| Startup exchange succeeds, fails, or times out | Log phase, elapsed time and outcome | Startup timing test covers all three with an injected clock |
| Timing logs enabled by default | Keep unrelated SDK info disabled | Gateway logging filter test |

## Alternatives considered

- **Move the gateway into the desktop app:** unnecessary for this fix; retain
  the independent service that already exists.
- **Keep Background and increase timeouts:** waiting longer does not remove the
  measured scheduling penalty. Timeouts stay unchanged.
- **Change Starting/Thinking/submission UI:** the owner excluded this work.
- **Add a pool of prestarted agents:** more resource and cleanup complexity,
  without enough evidence to justify it. Existing live sessions are reused.
- **Add a telemetry platform or retain raw provider output:** outside this small
  diagnostic change. Raw output may contain secrets. The broader telemetry
  proposal remains [ADR 195](../todo/195-tracing-is-the-telemetry-port.md).

## Consequences and follow-up work

Interactive scheduling may increase competition with other apps. Measure idle
CPU, wakeups, memory and energy use, and realistic concurrent request load after
installation. Use the phase logs to choose any further startup optimizations.
The measured warm improvement is not a battery or cold-start result.

Linux keeps its systemd user service; its generated unit does not request the
macOS Background policy or an explicit low CPU priority. Windows managed gateway
startup remains unimplemented in the reviewed checkout; its Task Scheduler proof
is separate work. Neither platform's launch implementation changes here.

This PR changes source. Installing a rebuilt app is a later step; the existing
running production service was not edited or restarted during the investigation.
