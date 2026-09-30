import { useEffect, useRef, useState } from "react"
import {
  lineage,
  runById,
  settled,
  verdictReason,
  type Experiment,
  type Run,
  type Score,
} from "../model/experiment"
import { dollars, percentChange, points, score } from "./format"
import { ago, count, exact } from "../../ui/format"
import { tooltip } from "../../ui/tooltip"
import { AgentAvatar, AreaMark, Meter, VerdictPill } from "./parts"
import { CaseResultsView } from "./case-results"
import { ChangeView } from "./change-files"

/** A step back along the way a run was reached: the view, then each run followed. */
export interface Crumb {
  readonly label: string
  readonly onSelect: () => void
}

/**
 * One run, opened from anywhere in the experiment and filling it: what
 * happened to it, which test cases it moved, what changed and why, and what
 * it was built on. The way here is a breadcrumb — the view it was opened
 * from, then each run followed through its lineage — and Escape steps back
 * along it.
 */
export function RunDetail({
  experiment,
  runId,
  crumbs,
  onFollow,
  onBack,
  onOpenAgent,
}: {
  experiment: Experiment
  runId: string
  /** The way here, the view first; the run itself is not one of them. */
  crumbs: readonly Crumb[]
  onFollow: (runId: string) => void
  /** One step back: the run before, or the view. */
  onBack: () => void
  /** Opens the agent that made the run, in its conversation's subagents. */
  onOpenAgent?: (agentId: string) => void
}) {
  const run = runById(experiment, runId)
  const headRef = useRef<HTMLElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  // Each run opens at its top, with the caret on the way back.
  useEffect(() => {
    bodyRef.current?.scrollTo(0, 0)
    headRef.current?.querySelector("button")?.focus({ preventScroll: true })
  }, [runId])
  if (!run) return null
  const area = experiment.areas.find((each) => each.id === run.areaId)
  const agent = experiment.agents.find((each) => each.id === run.agentId)
  return (
    <section
      className="xp-detail"
      aria-label={`Run ${run.number}`}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation()
          onBack()
        }
      }}
    >
      <header className="xp-detail-head" ref={headRef}>
        <nav className="xp-crumbs" aria-label="Where this run was opened from">
          <ol>
            {crumbs.map((crumb, index) => (
              <li key={index}>
                <button type="button" className="xp-crumb" onClick={crumb.onSelect}>
                  {index === 0 ? (
                    <svg
                      width="12"
                      height="12"
                      viewBox="0 0 12 12"
                      aria-hidden
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.4"
                    >
                      <path d="M7.5 2.5L4 6l3.5 3.5" />
                    </svg>
                  ) : null}
                  {crumb.label}
                </button>
              </li>
            ))}
            <li aria-current="page">
              <span className="xp-crumb-here">
                {run.number === 0 ? "Baseline" : `Run ${run.number}`}
              </span>
              <VerdictPill verdict={run.verdict} />
            </li>
          </ol>
        </nav>
      </header>
      <div className="xp-detail-body" ref={bodyRef}>
        <h3 className="xp-detail-title">{run.title}</h3>
        <div className="xp-detail-meta">
          <AreaMark area={area} />
          {area?.name}
          <span className="xp-sep">·</span>
          {/* A run has no session of its own yet: it is a turn of its agent's
              conversation, so the name opens that agent's subagent, where the
              run is one of its turns. When a run has a session of its own,
              the name opens it at this run. */}
          {onOpenAgent && agent ? (
            <button
              type="button"
              className="xp-detail-agent"
              onClick={() => onOpenAgent(agent.id)}
              {...tooltip(`Open ${agent.name}`)}
            >
              <AgentAvatar seed={run.agentId} size={16} />
              {agent.name}
            </button>
          ) : (
            <>
              <AgentAvatar seed={run.agentId} size={16} />
              {agent?.name}
            </>
          )}
          <span className="xp-sep">·</span>
          {when(run.startedAt, experiment.asOf)}
        </div>

        <Outcome experiment={experiment} run={run} />

        {run.cases ? (
          <section className="xp-detail-block">
            <h4>Test cases</h4>
            <CaseResultsView results={run.cases} />
          </section>
        ) : null}

        <section className="xp-detail-block">
          <h4>The change</h4>
          <ChangeView
            experimentId={experiment.id}
            runId={run.id}
            change={run.change}
            rationale={run.rationale}
          />
        </section>

        <Lineage key={run.id} experiment={experiment} run={run} onFollow={onFollow} />
      </div>
    </section>
  )
}

/** "just now", "4m ago", "2h 10m ago". */
function when(at: number, now: number): string {
  const said = ago(at, now)
  return said === "now" ? "just now" : `${said} ago`
}

/**
 * What happened to the run, first: while it is graded, how far along and how
 * long is left; once it is settled, the verdict's reason and the run against
 * the version it was proposed from.
 */
function Outcome({ experiment, run }: { experiment: Experiment; run: Run }) {
  const parent = run.parentId === null ? undefined : runById(experiment, run.parentId)
  if (run.progress) {
    const { done, total } = run.progress
    const spent = Math.max(experiment.asOf - run.startedAt, 1)
    const left = done > 0 ? ((total - done) / done) * spent : null
    return (
      <section className="xp-detail-block" data-verdict={run.verdict}>
        <div className="xp-detail-progress">
          <span>
            <strong {...tooltip(exact(done))}>{count(done)}</strong>
            <span className="xp-faint"> / {count(total)} cases</span>
          </span>
          <span className="xp-faint">{left === null ? "Starting" : remaining(left)}</span>
        </div>
        <Meter value={done / total} label="Cases graded" />
        <p className="xp-detail-note">
          It is compared with{" "}
          {parent?.number === 0
            ? "the baseline"
            : `run ${parent?.number ?? "its parent"}`}{" "}
          when the last case is in.
        </p>
      </section>
    )
  }
  if (run.verdict === "queued")
    return (
      <section className="xp-detail-block" data-verdict={run.verdict}>
        <p className="xp-detail-note">{verdictReason(experiment, run)}</p>
      </section>
    )
  return (
    <section className="xp-detail-block" data-verdict={run.verdict}>
      <p className="xp-detail-reason">{verdictReason(experiment, run)}</p>
      {settled(run) && parent && settled(parent) ? (
        <>
          <Dumbbells
            rows={[
              { label: "Test", before: parent.test, after: run.test },
              { label: "Train", before: parent.train, after: run.train },
            ]}
            noise={experiment.noise}
          />
          <p className="xp-detail-note">
            Against {parent.number === 0 ? "the baseline" : `run ${parent.number}`}
            {run.cost && parent.cost ? (
              <>
                {" "}
                · cost per task {dollars(parent.cost)} → {dollars(run.cost)} (
                {percentChange(parent.cost, run.cost)})
              </>
            ) : null}
          </p>
        </>
      ) : null}
    </section>
  )
}

/** "40s left", "3m left". */
function remaining(ms: number): string {
  const seconds = Math.max(1, Math.round(ms / 1000))
  if (seconds < 60) return `${seconds}s left`
  return `${Math.round(seconds / 60)}m left`
}

/** How many steps of a long lineage stay in view before the rest fold away. */
const lineageShown = 3

/**
 * The versions this run was built on, from the baseline. A long lineage
 * keeps its ends in view — where it began, and the last few steps — and folds
 * the middle behind a count that opens it.
 */
function Lineage({
  experiment,
  run,
  onFollow,
}: {
  experiment: Experiment
  run: Run
  onFollow: (runId: string) => void
}) {
  const chain = lineage(experiment, run)
  const [open, setOpen] = useState(false)
  const folds = !open && chain.length > lineageShown + 2
  const hidden = folds ? chain.length - 1 - lineageShown : 0
  const steps = folds ? [chain[0], ...chain.slice(-lineageShown)] : chain
  const step = (each: Run) => (
    <li key={each.id}>
      <button
        type="button"
        className="xp-lineage-step"
        aria-current={each.id === run.id || undefined}
        onClick={() => onFollow(each.id)}
      >
        <span className="xp-truncate">{each.number === 0 ? "Baseline" : each.title}</span>
        <span className="xp-lineage-score">
          {each.test ? score(each.test.mean) : "…"}
        </span>
      </button>
    </li>
  )
  return (
    <section className="xp-detail-block">
      <h4>Built on</h4>
      <ol className="xp-lineage">
        {step(steps[0])}
        {folds ? (
          <li>
            <button
              type="button"
              className="xp-lineage-fold"
              onClick={() => setOpen(true)}
            >
              {hidden} earlier {hidden === 1 ? "step" : "steps"}
            </button>
          </li>
        ) : null}
        {steps.slice(1).map(step)}
      </ol>
    </section>
  )
}

function Dumbbells({
  rows,
  noise,
}: {
  rows: readonly { label: string; before: Score; after: Score }[]
  noise: number
}) {
  const values = rows.flatMap((row) => [
    row.before.mean - row.before.ci,
    row.after.mean - row.after.ci,
    row.before.mean + row.before.ci,
    row.after.mean + row.after.ci,
  ])
  const lo = Math.floor(Math.min(...values) - 1)
  const hi = Math.ceil(Math.max(...values) + 1)
  const at = (value: number) => `${((value - lo) / (hi - lo)) * 100}%`
  return (
    <div className="xp-dumbbells">
      {rows.map((row) => {
        const change = row.after.mean - row.before.mean
        const tone = change > noise ? "up" : change < -noise ? "down" : "flat"
        return (
          <div key={row.label} className="xp-dumbbell">
            <span className="xp-label">{row.label}</span>
            <span className="xp-dumbbell-track" data-tone={tone}>
              <span
                className="xp-dumbbell-noise"
                style={{
                  left: at(row.before.mean - noise),
                  width: `calc(${at(row.before.mean + noise)} - ${at(row.before.mean - noise)})`,
                }}
              />
              <span
                className="xp-dumbbell-ci"
                style={{
                  left: at(row.after.mean - row.after.ci),
                  width: `calc(${at(row.after.mean + row.after.ci)} - ${at(row.after.mean - row.after.ci)})`,
                }}
              />
              <span
                className="xp-dumbbell-link"
                style={{
                  left: at(Math.min(row.before.mean, row.after.mean)),
                  width: `calc(${at(Math.max(row.before.mean, row.after.mean))} - ${at(Math.min(row.before.mean, row.after.mean))})`,
                }}
              />
              <span
                className="xp-dumbbell-before"
                style={{ left: at(row.before.mean) }}
              />
              <span className="xp-dumbbell-after" style={{ left: at(row.after.mean) }} />
            </span>
            <span className="xp-dumbbell-values xp-mono">
              <span className="xp-faint">{score(row.before.mean)}</span> →{" "}
              {score(row.after.mean)}{" "}
              <span className="xp-delta" data-tone={tone}>
                {points(change)}
              </span>
            </span>
          </div>
        )
      })}
    </div>
  )
}
