# 333. An experiment is drawn from its definition, so any kind of experiment reads the same way

## Purpose

A conversation can run an experiment: a swarm of agents trying changes to a
product against a metric, keeping what improves it
([background](https://claude.dev/blog/automating-eval-design-and-hillclimbing/#eval-design)).
This record settles how the desktop window shows one — the climb, the areas
explored, every run, one run in detail — so that the same views serve a
percent score that should rise, a latency that should fall, or an A/B test
with no areas at all. It builds on widgets ([326](326-widgets.md)) for where
an experiment appears and on subagents ([329](329-subagents.md)) for its swarm.

- **Date:** 2026-09-30
- **Status:** proposed

## Context

The prototype (`exp-prototype` @ `5bfaa225`, `src/desktop/experiments/`) is
the look and the flow people signed off: a card in the conversation, an
overview with the climb and the path to the best version, an exploration map
by area, a runs table, and a run opening over any of them with a breadcrumb
back. Every word and number in it is the one sample's:

- one metric, in percent, higher is better, to one decimal, deltas in "pts";
- a train/test split on every run, with *overfit* as a verdict;
- a cost-per-task guardrail in dollars, with *costly* as a verdict;
- a noise floor, a ceiling "best model, max effort", and a run budget;
- five areas with glyphs looked up by the sample's area ids;
- verdict reasons composed as English sentences in the model.

The `Experiment.metric` string exists and nothing reads it. A second kind of
experiment would mean a second set of views.

What binds:

- **The harness decides, the window shows.** Whether a run is kept, and why a
  run was judged as it was, is the experiment runner's decision. The window
  must not re-judge runs from their numbers (gate 13).
- **The definition travels as data.** When the gateway serves experiments, the
  definition arrives over the wire; it cannot carry functions.
- **Scale.** One run can move a million cases and touch ten thousand files;
  views read aggregates and page or virtualise lists, never draw them whole.
- **No gateway source yet.** Until there is one, experiments come from
  in-memory samples, offered only under the preview.

## Decision

An **`ExperimentDefinition`** comes with every experiment from its source:

```ts
interface ExperimentDefinition {
  /** The metric the climb is on. */
  readonly metric: Metric
  /** What each run is scored on; exactly one is primary (the climb's). */
  readonly splits: readonly { id: string; label: string; primary?: true }[]
  /** Limits a run must stay within, each on a metric of its own. */
  readonly guardrails: readonly { metric: Metric; limit: Limit }[]
  /** The verdicts runs are given, in the order a filter lists them. */
  readonly verdicts: readonly {
    id: string
    label: string
    tone: "good" | "bad" | "neutral" | "warning" | "active"
    /** A kept run became the new best. */
    kept: boolean
    /** Still running or queued: no scores yet. */
    pending: boolean
  }[]
  readonly noise?: number
  readonly ceiling?: { value: number; label: string }
  readonly budget?: { runs: number }
  /** What a case is called ("test case", "prompt", "request"). */
  readonly caseNoun?: { one: string; other: string }
}

interface Metric {
  id: string
  name: string
  /** Shown after a value: "%", "ms", "$". */
  unit: string
  /** Shown after a change, when it differs from `unit`: "pts". */
  deltaUnit?: string
  better: "up" | "down"
  decimals: number
}

type Limit =
  | { kind: "at-most"; value: number }
  | { kind: "change-at-most"; ratio: number }
```

Which sections an experiment has follows from its data, not from flags: no
areas, no exploration map or area cards; no agents, no swarm; no cases or no
change on a run, no such block in its detail; a guardrail, a column and a
tile. Formatting a value or a change is derived from `Metric` in one place
(`model/metric.ts`), and every view that shows a number uses it.

**Runs** carry `scores` by split id and `guardrails` by metric id, a
`verdict` id from the vocabulary, and the harness's `reason` for it, as text.
The **champion** is the last kept run, or the baseline: *kept* already means
"became the new best", so the window never compares numbers to find it, and
direction does not enter into it. The best-so-far line is the kept runs in
order.

**Areas** carry their glyph as data (a path on a 16-unit grid) and a series
hue; nothing is keyed on a known area id. **Agents** are the swarm; their
conversations are subagents through an adapter to `SubagentSource`
(`adapters/subagents/`), so the subagents panel shows them like any other and
clicking an agent opens it there. A run has no session of its own yet, so an
agent opens its conversation, where the run is one of its turns.

**`ExperimentSource`** is the port: `get(id)`, `forSession(sessionId)` (a
conversation may run several), `subscribe`, and `openFile({ experimentId,
runId, path? }) → Opened` — `opened`, or `refused` with a typed reason shown
for a moment where the file was clicked. Nothing is retried.

**Components take their words as props.** Every heading, subtitle and label a
composite draws is a prop with a default derived from the definition (the
climb's title is the metric's name; a gain reads in `deltaUnit`), so a host
can say it differently without a fork. Primitives come from nessa_ui (#103,
#104, #105): `Meter`, `Delta`, `Stat`, `StatusLabel`, `AvatarStack`,
`Breadcrumb`, `EmptyState`, the glass `SegmentedControl`, `Sparkline`,
`ChartTooltip`, `ProportionBar`, and the existing `Card`, `Table`,
`FileDiffList`, `DiffStat`, `VirtualList`. The climb chart and the exploration
map stay here, built from them, with their geometry as pure functions.

**Where it appears** is a widget (326), plugin `experiments`: a card in the
conversation's message, the surface beside it, in its own pane, or filling
the window; Escape steps back along the run trail, then out. Navigation — the
view, the trail of runs followed, scroll and focus on opening one — is one
hook, `useExperimentNavigation`; the views only render.

**Scale**: cases are counts by slice with at most the source's page of moved
cases (and the total); files are a virtualised tree with search. Both are
measured at 1M cases and 10K files in the browser checks.

**Samples**, under the preview: the checkout-support hill-climb (percent, up,
train and test, a cost guardrail, five areas) and a second of another kind —
p95 latency in milliseconds, lower is better, one split, no areas — which
exists to prove this record: if a view needs a change to show it, the
definition is missing something.

## Alternatives considered

- **A view per kind of experiment.** Quickest for the second kind; the third
  would be a third copy, each drifting in look and in rules.
- **Formatting functions in the definition.** Flexible, but a definition must
  arrive over the wire; a unit, a delta unit and a precision cover every
  metric in view, and a new need adds a field.
- **Section flags in the definition.** Explicit, but a flag can say "areas"
  while the data has none, and then the view must decide which to believe.
  Deriving sections from the data leaves one answer.
- **The window judging runs.** Computing *kept*, *overfit* or *costly* from
  scores and guardrails. It lost because the harness already decides, with
  information the window does not have (confidence intervals, reruns), and two
  judges will disagree.

## Consequences

- A new kind of experiment is a definition and a source; if it needs a view
  change, that is a gap in this record, found by the second sample first.
- The prototype's 3K-line stylesheet and private palette go; each component
  has its stylesheet on the desktop's and nessa_ui's tokens.
- The views cannot say anything the definition or the harness did not: a
  reason the harness leaves out is not invented.
- Remaining: the gateway's `ExperimentSource`; opening a run's own session
  once runs have one; an editor inside the window, which would change
  `openFile` from handing off to showing.

## Work

| Issue | Scope |
| --- | --- |
| #334 | The definition, `model/metric.ts`, the model on it, the port, both samples and the scale sample, the preview |
| #335 | The components: climb, exploration map, area card, verdict, cases, change |
| #336 | The composites and the surface, the inline card, `useExperimentNavigation`, `experiments.mjs` |
| #337 | The swarm as subagents, and an agent opening its subagent |

Part of #325.
