# 329. A conversation's subagents are a vertical of their own, each read as a conversation

## Purpose

A conversation can put other agents to work — an experiment's swarm is one
case ([333](333-experiments.md)), a fan-out of reviewers or researchers is
another. This record settles what the desktop window knows of those
subagents, where it reads them from, and how a person sees them: in a panel of
the conversation's own, each subagent's work read as a conversation is. It
knows no experiment.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

The prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/subagents/`) showed
the shape people wanted: the conversation's subagents as an avatar stack in
its pane header, a panel listing every one — its avatar, its name with the
tags it was spun up with beside it, what it is doing now, a tagline — and one
subagent opening as an ordinary conversation, with a composer. Its only source
was an experiment's swarm read as subagents, which is why this vertical is
settled on its own before either consumer builds on it.

What binds:

- **Independent of experiments.** Any conversation may have subagents. The
  subagents vertical imports no experiment; the experiments vertical depends
  on this one (326, *Boundaries*, held by `desktop-verticals.mjs`).
- **Nothing can deliver a message to a subagent yet.** The gateway does not
  report a conversation's subagents, and an experiment's harness takes no
  messages for its agents. The window has in-memory samples, and must not
  look as if it could do what it cannot.
- **Sending has one design in the window** (238's outbox: a window-minted
  message id, `sent`, `refused`, `unknown`, Send Again and Discard, a message
  taken once per id). Writing to a subagent would be a second user of it, and
  two review rounds of this record found that doing so well means extracting
  that design from the workspace's state first — work worth doing against a
  source that can deliver, not a sample.

## Decision

`src/desktop/subagents/` owns the model, the port, the panel, and the session
accessory. It depends on the workspace's barrel for the conversation it draws —
transcript messages and views, the clock, the time labels — which the barrel
exports for it (#330, #331); never the other way. Counts and plurals ("6
agents", "1 case") are the desktop's, one pure module in `src/desktop/model/`
that this vertical and experiments both use.

**A subagent** is its identity (`id`, below), a `name`, the `seed` its
generated avatar is painted from (the same wherever it appears), the `tags` it
was spun up with (each an id, a name, a series hue `1 | 2 | 3 | 4 | 5` and a
glyph path — all data, none keyed on a known list), a `state`, a one-sentence
`headline`, `since`, the `work` it is on with its progress when known, the
`model` it runs on (shown), and its `conversation`: the workspace's transcript
messages and live activity line. **States** are the few a person reads
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

**Identity.** A subagent's id is unique within its conversation by construction:
composition joins sources (the in-memory sample and the experiments adapter
today, the gateway's later) into one `SubagentSource`, given as a `Record` from
each source's key to the source, so a key cannot repeat, and the join makes
every id its source's key, `:`, and the source's own id through the desktop's
one id encoder (326). Two sources cannot produce one id, and each source keeps
its own ids unique for a conversation — the experiments adapter through 333's
validation of agent ids; the join drops a repeated id from one source and logs
it as a fault. The join answers `unread` for a conversation while any of its
sources has not read it, then `ready` with every source's subagents, so a list
is never shown short.

**`SubagentSource`** is the port: `forSession(sessionId)`, answering from what
the source holds now (a view may read it on every render) `{ kind: "unread" }`
until the source has read that conversation's subagents, then `{ kind: "ready",
subagents }`, and `subscribe`.
It has no `send`: a subagent's conversation is read, not written to, until a
source can deliver.

**Where it is seen.** The panel is a widget (326), plugin `subagents`, whose id
is the conversation's session id, in a pane beside the conversation or over the
panes. Its `useWidget` answers, in this order: `off` while the preview is off;
`missing` for a conversation the workspace does not list once it has read its
index (removed, or never known), asked through a selector the workspace's barrel
exports for it (#330); `unread` while the workspace has not read its index or
the source has not read the conversation; otherwise `ready`, titled "Subagents"
with the conversation as its `origin` — one without subagents shows an empty
state. Which subagent it shows is this vertical's state, one per conversation
(`sessionId → subagentId | null`). The joined id is built in one place,
`joinedSubagentId(sourceKey, sourceId)`, which this vertical exports; each
source declares its own key (the experiments adapter's is its own constant,
which composition registers it under), so no other vertical rebuilds the rule.
The vertical exports `useOpenSubagent(host)`, given the calling view's host
callbacks and place (326 hands them to every view), and returning `(target: {
sessionId, sourceKey, sourceId }) => void`, which sets the subagent shown and
opens the panel through `host.openWidget` in the caller's place: from a view in
the window, in the window; from anywhere else, in a pane beside the conversation
— which is how an experiment's agent opens its subagent. The plugin's
`SessionAccessory` draws the conversation's subagents as an avatar stack in its
pane header, in the model's order (the busiest first), and nothing when there
are none; a click opens the panel.

**One subagent** is drawn as a conversation is — its messages and its live
activity line through the workspace's transcript views — with no composer.

**The preview.** Subagents are offered only when their preview is on under
Settings › Advanced › Experimental. The switch is a window preference
(`src/desktop/adapters/window-preferences.ts`, as the greeting's is) and its
entry is the settings catalogue's, the one owner of what Settings names; this
vertical reads the preference through its hook and owns neither. Off, no
accessory; an open panel answers `off` (326); a consumer's link to a subagent is
not offered. The in-memory sample serves the sample workspace's conversations.

## Alternatives considered

- **Messaging subagents now.** The prototype's composer. Against a sample it
  would only pretend to deliver, the experiment's harness has nowhere to put
  a message, and doing it honestly means sharing 238's outbox first; it waits
  for a source that can deliver.
- **Subagents as sessions in the workspace.** Each a hidden session with its
  own pane. It lost because a session carries retention, drafts, approvals and
  a place in the list and the overview, none of which a subagent has, and
  because the conversation a person opened is the parent.
- **A subagent view inside each consumer.** The experiment drawing its agents'
  conversations. The next consumer would draw them again, and the two would
  disagree about state and order.
- **A free status string.** A person reads stuck differently from planning,
  and a free string cannot be ordered.
- **Taglines from the model.** Cost and noise for decoration.

## Consequences

- Any source of subagents appears in one panel, with one order, read the way a
  conversation is.
- The sample is visible only under the preview; nothing ships that looks live
  and is not, or looks writable and is not.
- Remaining: the gateway's `SubagentSource`, when the gateway reports a
  conversation's subagents; then writing to a subagent — `send` with a message
  id taken once, on 238's outbox extracted into pure functions both the
  workspace and this vertical use — decided in its own record; choosing a
  subagent's model, once a source can.
- Work: #330 (the model, taglines, the port and the join, the sample, the
  preview, the time labels added to the workspace's `model/time-labels.ts`,
  the module map in `docs/codebase-structure.md`), #331 (the panel,
  `subagents.mjs`), #332 (the session accessory). Part of #325.
