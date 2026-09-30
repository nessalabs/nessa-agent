import { useMemo } from "react"
import { useExperiment } from "../adapters/react/experiments-provider"
import {
  bestSoFar,
  champion,
  runById,
  settled,
  summarizeArea,
  totals,
  type Experiment,
  type Run,
} from "../model/experiment"
import { points, score } from "./format"
import { AreaMark, Meter, Swarmpile } from "./parts"
import { LiveStatus } from "./live-status"
import { useWidth } from "./use-width"
import "./experiments.css"

/**
 * An experiment inside a conversation: where it stands, the climb in a line,
 * which areas the gains came from, and what the swarm is doing — enough to
 * know whether to look closer, and one click to open it beside the chat.
 */
export function ExperimentCard({
  id,
  opened,
  onOpen,
}: {
  id: string
  opened: boolean
  /** `pane` gives it a pane of its own; `beside` opens it inside the conversation's. */
  onOpen: (where: "beside" | "pane") => void
}) {
  const experiment = useExperiment(id)
  if (!experiment) return null
  const best = champion(experiment)
  const baseline = experiment.baseline.test?.mean ?? 0
  const count = totals(experiment)
  return (
    <article
      className="xp-inline"
      data-opened={opened || undefined}
      aria-label={experiment.title}
    >
      <header className="xp-inline-head">
        <span className="xp-kind">Experiment</span>
        <LiveStatus experiment={experiment} />
      </header>
      <h4 className="xp-inline-title">{experiment.title}</h4>
      <div className="xp-inline-figure">
        <div>
          <div className="xp-inline-value">
            {score(best.test.mean)}
            <span className="xp-hero-unit">%</span>
            <span className="xp-hero-delta" data-tone="up">
              {points(best.test.mean - baseline)}
            </span>
          </div>
          <span className="xp-label">
            best on test · from {score(baseline)} · {count.kept} kept of {count.settled}
          </span>
        </div>
        <Sparkline experiment={experiment} />
      </div>
      <GainsByArea experiment={experiment} />
      <footer className="xp-inline-foot">
        <LeadingRun experiment={experiment} />
        <button
          type="button"
          className="xp-open"
          aria-pressed={opened}
          title="⌥-click to open beside the conversation"
          onClick={(event) => onOpen(event.altKey ? "beside" : "pane")}
        >
          {opened ? "Showing" : "Open experiment"}
          <svg
            width="12"
            height="12"
            viewBox="0 0 12 12"
            aria-hidden
            fill="none"
            stroke="currentColor"
            strokeWidth="1.4"
          >
            <path d="M4.5 2.5L8 6l-3.5 3.5" />
          </svg>
        </button>
      </footer>
    </article>
  )
}

function Sparkline({ experiment }: { experiment: Experiment }) {
  const [ref, width] = useWidth<HTMLDivElement>(200)
  const height = 56
  const shape = useMemo(() => {
    const climb = bestSoFar(experiment)
    const done = experiment.runs.filter(settled)
    const last = Math.max(
      ...experiment.runs
        .filter((run) => run.verdict !== "queued")
        .map((run) => run.number),
      1,
    )
    const values = [
      ...done.map((run) => run.test.mean),
      experiment.baseline.test?.mean ?? 0,
    ]
    const lo = Math.min(...values) - 1
    const hi = Math.max(...values) + 1
    const x = (n: number) => 4 + (n / last) * (width - 12)
    const y = (v: number) => 4 + (1 - (v - lo) / (hi - lo)) * (height - 8)
    let line = ""
    climb.forEach((point, index) => {
      line +=
        index === 0
          ? `M${x(point.number)},${y(point.best)}`
          : `H${x(point.number)}V${y(point.best)}`
    })
    line += `H${x(last)}`
    const end = climb[climb.length - 1]
    return {
      line,
      area: `${line}V${height}H${x(0)}Z`,
      dots: done.map((run) => ({
        id: run.id,
        cx: x(run.number),
        cy: y(run.test.mean),
        kept: run.verdict === "kept",
      })),
      end: { cx: x(last), cy: y(end.best) },
    }
  }, [experiment, width])
  return (
    <div className="xp-sparkline" ref={ref} aria-hidden>
      <svg width={width} height={height}>
        <path d={shape.area} className="xp-sparkline-wash" />
        {shape.dots.map((dot) => (
          <circle
            key={dot.id}
            cx={dot.cx}
            cy={dot.cy}
            r={dot.kept ? 2.5 : 1.5}
            data-kept={dot.kept || undefined}
          />
        ))}
        <path d={shape.line} className="xp-sparkline-line" />
        <circle
          cx={shape.end.cx}
          cy={shape.end.cy}
          r={3.5}
          className="xp-sparkline-end"
        />
      </svg>
    </div>
  )
}

function GainsByArea({ experiment }: { experiment: Experiment }) {
  const summaries = experiment.areas.map((area) => summarizeArea(experiment, area))
  const total = summaries.reduce((sum, each) => sum + Math.max(each.gain, 0), 0)
  return (
    <div className="xp-gains">
      <div
        className="xp-gains-bar"
        role="img"
        aria-label="Where the gain came from, by area"
      >
        {summaries
          .filter((each) => each.gain > 0)
          .map((each) => (
            <span
              key={each.area.id}
              style={{
                flexGrow: each.gain / Math.max(total, 0.1),
                background: `var(--xp-area-${each.area.slot})`,
              }}
            />
          ))}
      </div>
      <ul className="xp-gains-legend">
        {summaries
          .filter((each) => each.gain > 0)
          .map((each) => (
            <li key={each.area.id}>
              <AreaMark area={each.area} size={12} />
              {each.area.name}
              <span className="xp-mono">{points(each.gain)}</span>
            </li>
          ))}
        {summaries.some((each) => each.gain <= 0) ? (
          <li className="xp-gains-rest">
            {summaries.filter((each) => each.gain <= 0).length} more without a gain yet
          </li>
        ) : null}
      </ul>
    </div>
  )
}

/** The evaluation closest to finishing, and the swarm behind it. */
function LeadingRun({ experiment }: { experiment: Experiment }) {
  const share = (run: Run) => (run.progress ? run.progress.done / run.progress.total : 0)
  const live = experiment.runs
    .filter((run) => run.verdict === "running" && run.progress)
    .sort((a, b) => share(b) - share(a))
  const lead = live[0]
  const agent = lead
    ? experiment.agents.find((each) => each.id === lead.agentId)
    : undefined
  return (
    <div className="xp-inline-lead">
      <Swarmpile agents={experiment.agents} size={18} />
      {lead?.progress && agent ? (
        <span className="xp-inline-lead-text">
          <span className="xp-truncate">
            {agent.name} · <span className="xp-mono">#{lead.number}</span>{" "}
            {runById(experiment, lead.id)?.title}
          </span>
          <Meter
            value={lead.progress.done / lead.progress.total}
            label="Closest evaluation"
          />
        </span>
      ) : (
        <span className="xp-muted">The swarm is between runs</span>
      )}
    </div>
  )
}
