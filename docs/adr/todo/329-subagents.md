# 329. A conversation's subagents are a vertical of their own, read and written to like conversations

## Purpose

A conversation can put other agents to work — an experiment's swarm is one
case ([333](333-experiments.md)), a fan-out of reviewers or researchers is
another. This record settles what the desktop window knows of those
subagents, where it reads them from, and how a person sees and talks to one:
in a panel of the conversation's own, each subagent read and written to as a
conversation is. It knows no experiment.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

The prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/subagents/`) showed
the shape people wanted: the conversation's subagents as an avatar stack in
its pane header, a panel listing every one — its avatar, its name with the
tags it was spun up with beside it, what it is doing now, a tagline — and one
subagent opening as an ordinary conversation with a composer. Its only source
was an experiment's swarm read as subagents, which is why this vertical is
settled on its own before either consumer builds on it.

What binds:

- **Independent of experiments.** Any conversation may have subagents. The
  subagents vertical imports no experiment; the experiments vertical depends
  on this one (326, *Boundaries*, enforced by `desktop-verticals.mjs`).
- **One way to send.** The window already sends a message to a conversation
  (238): the window mints the message's id, holds it in the outbox as
  sending, answers `sent`, `refused` or `unknown`, says "Not sent. …" with
  Send Again and Discard, and retires it when the conversation includes it.
  A subagent's composer is the window's composer, so it sends the same way.
- **No gateway source yet.** The gateway does not report a conversation's
  subagents. Until it does, the window has an in-memory sample and must not
  look as if it were live.

## Decision

`src/desktop/subagents/` owns the model, the port, the panel, and the session
accessory; it depends on the workspace's barrel for the conversation it draws
(transcript messages and views, the composer, the outbox rules, the clock and
the time labels), never the other way.

**A subagent** is its identity (`id`, below), a `name`, the `seed` its
generated avatar is painted from (the same wherever it appears), the `tags` it
was spun up with (each an id, a name, a series hue `1 | 2 | 3 | 4 | 5` and a
glyph path — all data, none keyed on a known list), a `state`, a one-sentence
`headline`, `since`, the `work` it is on with its progress when known, the
`model` it runs on, and its `conversation`: the workspace's transcript
messages and live activity line. **States** are the few a person acts on
differently:

| State | Means | Shown as |
| --- | --- | --- |
| `working` | doing a piece of work now; `work` says what | Working, with its progress when known |
| `planning` | between pieces of work, choosing the next | Planning |
| `stuck` | its line of work stalled; it is working out why | Stuck |
| `idle` | nothing left for it to do | Idle |

The order a list shows them in (working, the furthest along first, then
planning, stuck, idle) and the counts by state are the model's.

**Its tagline** is one line picked from its seed by a stable hash ("Quietly
judging your regexes"). It is decoration, so a crew reads as a crew; it is
drawn apart from the state and says nothing about it.

**Identity.** A subagent's id is unique within its conversation by
construction: composition joins sources (the experiments adapter today, the
gateway's later) into one `SubagentSource`, each under a key of its own, and
the join prefixes every id with its source's key. Two sources cannot produce
one id, and the join routes each call to the source that owns the subagent.

**`SubagentSource`** is the port: `forSession(sessionId)` answers from what the
source holds now (a view may read it on every render), `subscribe`, and
`send({ sessionId, subagentId, messageId, text }) → Promise<void>`, settling
as the workspace's `send` does — resolved when taken, rejected with a typed
refusal, or with `unavailable` when no answer came in the source's bound.
There is no model choice: a subagent's model is shown, not chosen, until a
source can change it.

**Sending** follows 238 row for row, with the outbox held by this vertical
above the panel, per subagent, per message (keyed by `messageId`), so it
survives the panel closing:

| Event | While | Outcome | Shown |
| --- | --- | --- | --- |
| person sends | — | the message joins the outbox, sending | the message, "sending" |
| another send | one is sending | the second joins the outbox beside it; each settles on its own | both, in order |
| source takes it | sending | `sent`; it stays in the outbox until the conversation includes it | the message |
| source refuses (typed) | sending | `refused` | "Not sent. …", with Send Again and Discard |
| no answer in the bound | sending | `unknown` | as refused, with the reason that no answer came |
| the conversation includes its id | any | retired from the outbox; a late answer to it is let go | the message, once |
| the panel closes | any | nothing changes | the outbox as it was when the panel opens again |
| the subagent or the conversation is removed | any | its outbox is let go | nothing |

The rule that retires a message the conversation includes is the workspace's
(`unconfirmed` in `workspace/model/transcript.ts`), used here, not
copied. Nothing is retried by the window; the tests of this table in #331
hold it.

**Where it is seen.** The panel is a widget (326), plugin `subagents`, whose id
is the conversation's session id: attached to the conversation's pane, in a
pane of its own, or over the panes. Which subagent it shows is this
vertical's state, one per conversation (`sessionId → subagentId | null`); the
vertical exports `useOpenSubagent()` — set it, then open the widget through
the host's `openWidget` — which is how an experiment's agent opens its
subagent. The plugin's `SessionAccessory` draws the conversation's subagents
as an avatar stack in its pane header, the busiest first, and nothing when
there are none; a click opens the panel.

**The preview.** Subagents are offered only when their preview is on under
Settings › Advanced › Experimental (its own switch, owned by this vertical's
catalogue entry). Off, no accessory, no panel, and a consumer's link to a
subagent is not offered. The in-memory sample serves the sample workspace's
conversations.

## Alternatives considered

- **Subagents as sessions in the workspace.** Each a hidden session with its
  own pane. It lost because a session carries retention, drafts, approvals and
  a place in the list and the overview, none of which a subagent has, and
  because the conversation a person opened is the parent.
- **A subagent view inside each consumer.** The experiment drawing its agents'
  conversations. The next consumer would draw them again, and the two would
  disagree about state and order.
- **Its own send states.** Simpler to write here; a second machine for one
  composer, which would disagree with 238 the first time either changed.
- **A free status string.** A person acts differently on stuck than on
  planning, and a free string cannot be ordered.
- **Taglines from the model.** Cost and noise for decoration; a fixed list
  keeps it decoration.

## Consequences

- Any source of subagents appears in one panel, with one order, and is
  written to the way a conversation is.
- The workspace's outbox rule becomes shared: a change to it changes both, as
  it should.
- The sample is visible only under the preview; nothing ships that looks live
  and is not.
- Remaining: the gateway's `SubagentSource`, when the gateway reports a
  conversation's subagents, with this record's send table as its contract;
  choosing a subagent's model, once a source can.
- Work: #330 (the model, taglines, the port and the join, the sample, the
  preview, the time labels it needs added to the workspace's
  `model/time-labels.ts`), #331 (the panel, the outbox and the send table,
  `subagents.mjs`), #332 (the session accessory). Part of #325.
