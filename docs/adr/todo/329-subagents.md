# 329. A conversation's subagents are a vertical of their own, read and written like chats

## Purpose

A conversation can put other agents to work — an experiment's swarm is one
case ([333](333-experiments.md)), a fan-out of reviewers or researchers is
another. This record settles what the desktop window knows of those
subagents, where it reads them from, and how a person sees and talks to one:
in a panel of the conversation's own, each subagent read like any chat. It
knows no experiment.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

The prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/subagents/`) showed
the shape people wanted: a face pile in the conversation's header, a panel
listing every subagent — its face, its name with the tags it was spun up with
beside it, what it is doing now, a line of character — and one subagent opening
as an ordinary conversation with a composer. Its only source was the
experiment's swarm read as subagents, which is why the vertical has to be
settled on its own before either consumer builds on it.

What binds:

- **Independent of experiments.** Any conversation may have subagents; the
  subagents vertical must not import the experiments vertical, and the
  experiments vertical reaches it only through its port.
- **No gateway source yet.** The gateway does not report a conversation's
  subagents today. Until it does, the window has in-memory samples and must say
  so rather than imply a live connection.
- **Sending can be lost.** A message to a subagent is an action with an answer
  that may not arrive; its states are written down before it is built (gate 15).

## Decision

`src/desktop/subagents/` owns the model, the port, and the views.

**The subagent** is its identity (`id`, unique within its conversation), a
`name`, the `seed` its generated face is painted from (the same wherever it
appears), the `tags` it was spun up with (id, name, a series hue 1–5, a glyph
path — all data, none keyed on a known list), a `state` (`working | thinking |
stuck | resting`), a one-sentence `headline`, `since`, the `work` it is on with
its progress when known, the `model` it runs on, and its `conversation` as the
workspace's own transcript messages plus the live activity line. The order a
list shows them in (`byActivity`) and the counts by state are the model's.

**Its character** is one line picked from its seed by a stable hash
("Quietly judging your regexes"). It is decoration so a crew reads as a crew;
nothing reads meaning into it, and it is never shown as status.

**`SubagentSource`** is the port: `forSession(sessionId)` answers from what the
source holds now (so a view may read it on every render), `subscribe`, and
`send(sessionId, subagentId, text) → Sent`. Composition may join several
sources into one (the experiments adapter, later the gateway's); a subagent id
seen from two sources for one conversation is the second source's defect,
reported to diagnostics and not shown twice.

**Sending**, from the composer in a subagent's conversation:

| State | Event | Next | Shown |
| --- | --- | --- | --- |
| idle | person sends | sending | the message, marked sending |
| sending | source accepts | sent | the message |
| sending | source refuses (typed reason) | idle | the message, with why it was not sent; the draft is kept |
| sending | no answer in the source's bound | unconfirmed | the message, "not confirmed — the conversation shows where it stands" |
| unconfirmed | the message appears in the subagent's conversation | sent | the message |

Nothing is retried by the window. The in-memory sample accepts at once and
appends a reply; the gateway source, when it exists, owns its bound.

**Where it is seen**: the panel is a widget ([326](326-widgets.md)) whose id is
the conversation's session id, opened beside the conversation or in a pane of
its own; which subagent it shows is held above the panes, so a click anywhere
(a face in the header, an agent in an experiment) can open it on one. The
conversation's pane header shows its subagents as an `AvatarStack`, the
busiest first, and nothing when there are none.

**Until the gateway reports subagents**, an in-memory sample source serves the
sample workspace's conversations, and the window offers subagents only when
the preview is on under Settings › Advanced › Experimental.

## Alternatives considered

- **Subagents as sessions in the workspace.** Each subagent a hidden session
  with its own pane. It lost because a session carries retention, drafts,
  approvals and a place in the list and the overview — none of which a
  subagent has — and because the parent conversation is what a person opened.
- **A subagent view inside each consumer.** The experiment drawing its own
  agents' conversations. It lost because the next consumer would draw them
  again, and the two would disagree about state and order.
- **A generic "status" instead of the four states.** It lost because a person
  acts differently on stuck than on thinking, and a free string cannot be
  ordered.
- **Character lines from the model.** Asking the subagent to describe itself.
  It lost for now on cost and noise; the line is decoration, and a fixed list
  keeps it so.

## Consequences

- Any source of subagents — the experiment's swarm today, the gateway's later —
  appears in one panel with one order and one way to talk to it.
- The composer in a subagent's conversation reuses the window's composer, so
  its model choice and send behave as a conversation's do; the source decides
  what a model change means for a subagent.
- The sample is visible only under the preview; nothing ships that looks live
  and is not.
- Remaining: the gateway's `SubagentSource`, once the gateway reports a
  conversation's subagents; this record's send table is its contract.

## Work

| Issue | Scope |
| --- | --- |
| #330 | The model, the character line, the port, the sample source, the preview, the shared time formatting |
| #331 | The panel: the list and one subagent's conversation, the send states above, `subagents.mjs` |
| #332 | The face pile in a conversation's pane header |

Part of #325.
