# Shared event progress in thread tools

Based on assembled `6a910a84c615cb0f1182fced8269cc54fe6ed366`, including the
application EventProgress extraction from API commit 70261dc8. Tools initialize
that owner from the requested EventStart and ask it about every read before
projecting Page metadata or replacing an Idle checkpoint. No comparison rule is
copied into the tool adapter. Design rows were updated before implementation.

Two actual-follow tests exercise 22 ordered cases through a SessionStorage
adapter that deliberately ignores EventStart. After rejects wrong incarnation,
backwards Page/Idle, duplicate Page, and Page at its retained outer boundary.
It accepts a later change within the same unit, a later unit, and an equal Idle.
Now and Beginning establish continuity at Idle, retain it across Preparing,
reject contradictory subsequent Page/Idle, and accept trailing events after an
equal Idle. A normal Now first Page at its source's captured position remains
accepted. SessionChange pages are structurally valid local settlements, so a
history-gap error cannot mask the missing correlation guard.

Mutation: removed only the tool's progress.accept call, retaining the shared
owner and final target authorization. Both new follow tests failed (exit 101),
each receiving successful turn.ended metadata and a replacement-incarnation
cursor instead of event_cursor_invalid. The edit was asserted and its hunk
captured before running; the exact green source was restored with a fresh mtime.

Restored serial verification on Linux, using /workspace/target:

- composition::conversation_tools:: — 12 passed.
- event_progress — four application-owned tests passed.
- event_read_reauthorizes_page_and_idle_before_publication — passed.
- agent_tool — eight source policy/audit/admission/Relay tests passed.
- Direct rustfmt and git diff whitespace checks passed.

Raw local logs: /tmp/953-progress-mutation.log and .diff,
/tmp/953-progress-restored-tools.log, -shared.log, -api-auth.log, and
-agent-tool.log. The coordinator retains assembled strict Clippy, E2E, and
fresh independent review.

## Replay field-presence follow-up

Based on e5c83352, the subscribe input check now rejects after with either
replay:true or replay:false, matching the existing mutual-exclusion contract.
The real Relay regression loops over both boolean values. Design evidence was
updated first; the production change is the single presence predicate.

A separate verified mutation changed only replay.is_some() back to
replay.unwrap_or(false). The true case still refused, but the false case returned
successful empty events and cursor; the Relay assertion failed with exit 101.
The corrected source was restored with a fresh mtime, then the complete actual
Relay regression passed. Direct rustfmt and git diff checks passed.
Logs: /tmp/953-replay-mutation.log, /tmp/953-replay-mutation.diff, and
/tmp/953-replay-restored.log. Previously passed progress/auth suites were not
repeated for this isolated input fix; assembled strict Clippy and E2E remain with
the coordinator.
