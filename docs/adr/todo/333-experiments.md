# 333. An experiment is drawn from its definition, so any kind of experiment reads the same way

## Purpose

A conversation can run an experiment: agents trying changes to a product against
a metric, keeping what improves it
([background](https://claude.dev/blog/automating-eval-design-and-hillclimbing/#eval-design)).
This record settles how the desktop window shows one — the climb, the areas
explored, every run, one run in detail — so that the same views serve a percent
score that should rise, a latency that should fall, or a comparison with no
areas and no swarm. It builds on widgets ([326](326-widgets.md)) for where an
experiment appears and on subagents ([329](329-subagents.md)) for its agents.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

The prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/experiments/`) is
the look and flow people signed off: a card in the conversation, an overview
with the climb and the path to the best version, an exploration map by area,
a runs table, and a run opening over any of them with a breadcrumb back. Every
word and number in it is the one sample's:

- one metric, in percent, higher is better, to one decimal, changes in "pts";
- a train/test split on every run, with *overfit* as a verdict;
- a cost-per-task guardrail in dollars, with *costly* as a verdict;
- a noise floor, a ceiling "best model, max effort", and a run budget;
- five areas, their glyphs looked up by the sample's area ids;
- verdict reasons composed as English sentences in the model;
- the best run found by taking the last kept run in array order.

`Experiment.metric` is a string nothing reads. A second kind of experiment
would mean a second set of views.

What binds:

- **The harness decides, the window shows.** Whether a run is kept, which run
  is best, and why a run was judged as it was, are the experiment runner's
  decisions, made with evidence the window does not have (intervals, reruns)
  and revisable by it. The window does not re-judge runs (gate 13).
- **The definition travels as data.** When the gateway serves experiments,
  the definition arrives over the wire; it cannot carry functions.
- **Scale.** A run can move a million cases and touch ten thousand files.
- **No gateway source yet.** Until there is one, experiments come from
  in-memory samples under the preview.

## Decision

### The definition

Every experiment arrives with its definition:

```ts
interface ExperimentDefinition {
  /** The metric the climb is on. */
  readonly metric: Metric
  /** What each run is scored on, and which of them the climb follows. */
  readonly splits: readonly { readonly id: string; readonly label: string }[]
  readonly primarySplit: string
  /** Limits a run must stay within, each on a metric of its own. */
  readonly guardrails: readonly {
    readonly id: string
    readonly metric: Metric
    readonly limit: Limit
  }[]
  /** The verdicts runs are given, in the order a filter lists them. */
  readonly verdicts: readonly {
    readonly id: string
    readonly label: string
    readonly tone: "good" | "bad" | "neutral" | "warning" | "active"
    /** Kept: became the new best. Rejected: settled and not kept. Pending: not decided yet. */
    readonly outcome: "kept" | "rejected" | "pending"
  }[]
  /** Changes within it read as neutral. */
  readonly noise?: number
  /** A value to draw a line at, with what it is ("best model, max effort"). */
  readonly reference?: { readonly value: number; readonly label: string }
  readonly budget?: { readonly runs: number }
  /** What a case is called: "test case", "prompt", "request". */
  readonly caseNoun?: { readonly one: string; readonly other: string }
}

interface Metric {
  readonly id: string
  readonly name: string
  /** After a value: "%", "ms", "$". */
  readonly unit: string
  /** After a change, when it differs from `unit`: "pts". */
  readonly deltaUnit?: string
  readonly better: "up" | "down"
  readonly decimals: number
}

/** `relativeTo: "baseline"` reads `value` as a ratio of the baseline's (1.1 = 10% above it). */
interface Limit {
  readonly bound: "at-most" | "at-least"
  readonly value: number
  readonly relativeTo?: "baseline"
}
```

**Runs** carry an `id`, `number` (the harness's label for the run, from 1,
higher for a later-made run; nothing but lineage reads an order into it),
`startedAt`, `settledAt` — when the run's outcome last changed: a keep decided
after reruns moves it, a rerun that confirms the same outcome does not; present
exactly when the outcome is not `pending` — `parentId` (what it was built on:
the baseline or an earlier run), `scores` by split id, each `{ mean, interval?
}` (the half-width of its confidence interval), `measures` by guardrail id,
their `verdict` id, the harness's `reason` for it as text, and optionally
`areaId`, `agentId`, `cases` and `change`. The **baseline** is a type of its own
— an `id`, `scores` and `measures`, nothing else — and not one of `runs`: it is
what runs are judged against, not a run judged, so it has no parent, verdict or
time to get wrong. The **experiment** carries its `id`, `title`, `goal`, the
`sessionId` of the conversation that runs it (a widget's `origin`), `startedAt`,
the harness's `notes` (each a tone, a text, a time, and the run it is about, if
any), the definition, its `areas` and `agents` (each with an `id`; an agent with
an optional `areaId` and an activity, which may name the run it is on), its
baseline and runs, and — from the harness — `bestSoFar`: the ids of the runs
that became the best, in the order they did, empty until one does. The best
version is its last entry, or the baseline while it is empty; there is no second
field naming it, so the two cannot disagree. A keep the harness withdraws leaves
`bestSoFar` in the harness's next word. The window draws the climb and the best
version from these and never finds either itself. The climb is drawn over time,
and its best-so-far series begins at the baseline's primary-split score at the
experiment's `startedAt` once the baseline is scored (at the first keep until
then): each settled run with a score on the primary split is a point at its
`settledAt`, and the best-so-far line steps at each `bestSoFar` run's
`settledAt` — for a kept run, when it became kept — so runs that finish out of
order, or are kept after reruns, never step it backwards. The card's sparkline
is the same series, drawn as given with no `better`, so it cannot disagree with
the climb. The path to the best version, and the exploration map's thread, are
`bestSoFar` in order; the runs table's newest-first is by `startedAt`, then
`number`, and its score sorts follow the metric's `better`; a run's lineage
(`parentId`) is drawn only in its own detail. A baseline is *scored* once it has
a score on the primary split; until then the views say it is being scored, and a
guardrail limited relative to it reads "not measured yet" until the baseline has
that measure.

**Validation** is at the source's adapter, where external data is parsed:

- **The definition**: split, guardrail and verdict ids each unique;
  `primarySplit` names a split; every metric's `decimals` an integer from 0 to
  10.
- **Identity**: ids unique across the baseline and runs together, and among
  areas and among agents; runs' `number`s unique and at least 1.
- **References**: every run's verdict in the vocabulary; every score's split and
  every measure's guardrail defined, the baseline's too; a run's `parentId` the
  baseline or a run with a lower `number`, so lineage cannot loop; `areaId`,
  `agentId`, an agent's `areaId` and its activity's run, and a note's run (the
  baseline or a run) resolve.
- **Settling**: a run's `settledAt` present exactly when its verdict's outcome
  is not `pending`; a run's `startedAt` at or after the experiment's, and its
  `settledAt` at or after its own `startedAt`.
- **The best**: `bestSoFar` names runs, none twice, each kept and scored on the
  primary split, their `settledAt` never decreasing along it, with every kept
  run in it.

An experiment that fails is not
drawn in part: the widget answers `unshowable` — "Can't show this here", 326's
table — and the adapter logs what was wrong as a fault, as the workspace's
`failureReason` does. What validation returns is a branded `Experiment` that
only `validateExperiment` makes, so a view cannot be handed one it did not
check.

**Sections follow the data.** No areas, no exploration map or area cards; no
agents, no swarm; no `cases` or `change` on a run, no such block in its
detail; guardrails, a column and a tile each; no `noise`, no band; no
`reference`, no line; no `budget`, no budget line in the status.

**A metric's numbers are formatted in one place**,
`experiments/model/metric.ts`, from a `Metric`: a value with its unit and
decimals, and a change as a branded `Change` — its value, the format of its size
(called with the absolute value), and its tone, `good`, `bad` or `neutral` by
`better` and `noise`. Values come back as a branded `Formatted` text; components
take `Formatted` for metric values and `Change` for changes, and draw a change
only by handing a `Change` to nessa_ui's `Delta`, which adds the sign and does
not judge the change. A component therefore holds no raw metric number to format
itself; a type test holds it. Counts are not metric values: they go through the
desktop's counts module (329).

**Words are props.** Every heading, subtitle and label a composite draws is a
prop, its default derived from the definition (the climb is titled by the
metric's name; a gain reads in `deltaUnit`), so a host can say it otherwise
without a fork. Primitives come from nessa_ui (nessalabs/nessa_ui#103,
nessalabs/nessa_ui#104, nessalabs/nessa_ui#105) — `Meter`, `Delta`, `Stat`,
`StatusLabel`, `AvatarStack`, `Breadcrumb`, `EmptyState`, the glass
`SegmentedControl`, `Sparkline`, `ChartTooltip`, `ProportionBar` — with the
existing `Card`, `Table`, `FileDiffList`, `DiffStat` and `VirtualList`. The
climb chart and the exploration map stay here, built from them, their geometry
pure functions.

### Areas, agents, cases and changes

**Areas** carry their glyph as data (a path on a 16-unit grid) and a series hue
`1 | 2 | 3 | 4 | 5`; nothing is keyed on a known area id. **Agents** are the
swarm, each with an activity; their conversations are subagents through an
adapter (`experiments/adapters/subagents/`) registered with 329's join under its
own key, the constant `experimentsSubagentKey` it exports, each subagent's id
the experiment's id and the agent's, each through the desktop's one id encoder
(326), joined by `/`, so two experiments in one conversation cannot collide. An
experiment whose `get` answers `invalid` gives no subagents and counts as read,
so it neither holds the join at `unread` nor goes unsaid: its own widget says it
cannot be shown. Clicking an agent opens its subagent (329's `useOpenSubagent`,
given that key and the agent's source id), offered only while subagents' preview
is on. A run has no session of its own yet, so an agent opens its conversation,
where the run is one of its turns.

**Cases** are counts: a total, how many a run fixed and broke, and a **slice** —
a named group of cases, such as a category — each with its total and its passing
count before and after; plus one page of moved cases from the source (at most
200) with the total that moved. **A change** is a summary and its files (path,
status, lines added and removed), drawn as a virtualised tree with search.
**`openFile({ experimentId, runId, path? })`** hands a file's diff to the
person's editor, or with no path the run's whole change; it answers `opened`, or
`refused` with a typed reason shown where it was clicked. One hook,
`useOpenFile`, owns the table below — the latest request per target (a file's
row, or the whole-change control) and the four seconds — and `ChangeView` takes
its state and callbacks as props. Both it and the adapter's bound run on an
injected timer, `after(ms, run) → cancel`, the `Schedule` shape composition
already builds for the in-memory workspace source (`dependencies.ts`), which
#334 lifts into `WorkspaceDependencies` and `ClockProvider` beside `now`,
overridable as `now` is — not a second timer. Nothing is retried, and every call
settles (the adapter answers `refused`, `unavailable`, when no answer comes in
its bound):

| At a target | Event | Next | Shown |
| --- | --- | --- | --- |
| idle | click | asked | nothing new |
| asked | click again | asked, for the new request | nothing new |
| asked | the latest request answers `opened` | idle | nothing |
| asked | the latest request answers `refused` | refused | its reason, for four seconds |
| asked | an earlier request answers | asked | nothing: it is let go |
| idle | an earlier request answers | idle | nothing: it is let go |
| refused | an earlier request answers | refused | nothing: it is let go |
| refused | four seconds pass | idle | the reason goes |
| refused | click | asked | the reason goes |

### The port, the places, the preview

**`ExperimentSource`**: `get(id)` answering `{ kind: "ready", experiment } | {
kind: "unread" } | { kind: "missing" } | { kind: "invalid" }` — which the
plugin's `useWidget` maps to 326's `ready`, `unread`, `missing` and `unshowable`
— `forSession(sessionId)` answering `{ kind: "unread" }` until the source has
read that conversation, then `{ kind: "ready", ids }` — exactly the experiments
whose `sessionId` is that conversation, derived from them in the adapter, not
held beside them (a conversation may run several, or none) — `subscribe`, and
`openFile`. An experiment appears as a widget (326), plugin `experiments`, in
all three places — its card inline, a pane beside the conversation, over the
panes. Navigation — the view, the trail of runs followed, scroll and focus on
opening one — is one hook, `useExperimentNavigation`, which registers 326's
`onEscape` while the trail is not empty; the views only render. Experiments are
offered only when their own preview is on under Settings › Advanced ›
Experimental — a window preference with its entry in the settings catalogue,
read through its hook as 329's is. A preview decides only whether the window
offers a way in (`docs/codebase-structure.md`): off, no experiment's card, pane
or window view is offered, and a widget already open answers `off`; the swarm's
subagents are subagents, offered or not by subagents' own preview.

**Samples**, under the preview: the checkout-support hill-climb (percent, up,
train and test, a cost guardrail relative to the baseline, five areas, a
swarm), and a second of another kind — p95 latency in milliseconds, lower is
better, one split, an accuracy guardrail at least a fixed value, no areas and
no swarm. The second exists to test this record: if a view needs a change to
show it, the definition is missing something. A scale sample moves a million
cases and touches ten thousand files in one run.

**Scale contract**, measured on the scale sample under the conditions
`perf-budget.mjs` states (a production build, Chromium at 4× CPU throttling,
through `lib/perf.mjs`): opening the run and scrolling its files and cases
keeps every frame within 50 ms, and the page draws at most the visible files
plus overscan. WebKit runs the same steps for behaviour, not frames.

## Alternatives considered

- **A view per kind of experiment.** Quickest for the second kind; the third
  would be a third copy, each drifting in look and in rules.
- **Formatting functions in the definition.** Flexible, but a definition must
  arrive over the wire; a unit, a delta unit and a precision cover every
  metric in view, and a new need adds a field.
- **Section flags.** A flag can say "areas" while the data has none, and then
  a view has to decide which to believe.
- **The window finding the best run.** The last kept run, or the best score in
  the metric's direction. Swarm runs finish out of order and a harness may
  withdraw a keep after reruns; either rule would be a second owner of "best".
- **The window judging runs** from scores and guardrails. The harness already
  decides, with evidence the window does not have.

## Consequences

- A new kind of experiment is a definition and a source; if it needs a view
  change, that is a gap in this record, and the second sample should find it
  first.
- The prototype's 3K-line stylesheet and private palette go; each component
  has its stylesheet on the desktop's and nessa_ui's tokens.
- The views say nothing the definition or the harness did not: a reason the
  harness leaves out is not invented.
- Remaining: the gateway's `ExperimentSource`; opening a run's own session
  once runs have one; an editor inside the window, which would change
  `openFile` from handing off to showing.
- Work: #334 (the definition and validation, `model/metric.ts`, the model, the
  port, the samples, the preview, the module map in
  `docs/codebase-structure.md`), #335 (the components), #336 (the composites,
  the surface, the inline card, navigation, `experiments.mjs`), #337 (the swarm
  as subagents). Part of #325.
