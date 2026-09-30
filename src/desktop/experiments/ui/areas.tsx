import { useMemo, useState } from "react"
import {
  summarizeArea,
  verdictLabels,
  type Experiment,
  type Run,
} from "../model/experiment"
import { ClimbTooltip } from "./climb-chart"
import { points } from "./format"
import { AgentAvatar, AreaMark, AreaStatePill, areaGlyph } from "./parts"
import { useWidth } from "./use-width"

/**
 * Where the swarm has looked: one lane per area across the same runs as the
 * climb, the kept changes threaded together into the path to the best, and a
 * card per area saying how it is going and who is on it.
 */
export function Areas({
  experiment,
  onPick,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
}) {
  return (
    <div className="xp-areas">
      <section className="xp-card xp-card-flush" aria-labelledby="xp-map-title">
        <header className="xp-card-head">
          <div>
            <h3 id="xp-map-title">Exploration map</h3>
            <p>
              Every change by area, in the order it was tried. The thread joins what was
              kept.
            </p>
          </div>
        </header>
        <ExplorationMap experiment={experiment} onPick={onPick} />
      </section>
      <div className="xp-area-grid">
        {experiment.areas.map((area) => (
          <AreaCard
            key={area.id}
            experiment={experiment}
            areaId={area.id}
            onPick={onPick}
          />
        ))}
      </div>
    </div>
  )
}

const lane = 44
const labelWidth = 148
const padRight = 24

function ExplorationMap({
  experiment,
  onPick,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
}) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const [hovered, setHovered] = useState<Run | null>(null)
  const height = experiment.areas.length * lane + 28
  const runs = experiment.runs.filter((run) => run.verdict !== "queued")
  const last = Math.max(...runs.map((run) => run.number), 1)
  const x = (number: number) =>
    labelWidth + (number / (last + 0.6)) * Math.max(width - labelWidth - padRight, 60)
  const laneOf = (areaId: string) =>
    Math.max(
      experiment.areas.findIndex((area) => area.id === areaId),
      0,
    )
  const y = (areaId: string) => 14 + laneOf(areaId) * lane + lane / 2

  const thread = useMemo(() => {
    const kept = runs.filter((run) => run.verdict === "kept")
    if (kept.length === 0) return ""
    let path = `M${x(0)},${y(kept[0].areaId)}`
    let px = x(0)
    let py = y(kept[0].areaId)
    for (const run of kept) {
      const nx = x(run.number)
      const ny = y(run.areaId)
      const mid = (px + nx) / 2
      path += `C${mid},${py} ${mid},${ny} ${nx},${ny}`
      px = nx
      py = ny
    }
    return path
    // Width drives `x`; the runs drive the rest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [experiment, width])

  return (
    <div className="xp-map" ref={ref} style={{ height }}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label="Changes tried in each area over time"
      >
        {experiment.areas.map((area, index) => (
          <g key={area.id} className="xp-map-lane">
            <rect
              x={0}
              y={14 + index * lane}
              width={width}
              height={lane}
              data-odd={index % 2 === 1 || undefined}
            />
            <svg
              x={13}
              y={14 + index * lane + lane / 2 - 7}
              width={14}
              height={14}
              viewBox="0 0 16 16"
              className="xp-area-mark"
              style={{ color: `var(--xp-area-${area.slot})` }}
            >
              {areaGlyph(area)}
            </svg>
            <text x={32} y={14 + index * lane + lane / 2} dy="0.32em">
              {area.name}
            </text>
          </g>
        ))}
        <path className="xp-map-thread" d={thread} />
        {thread ? (
          <circle
            className="xp-map-origin"
            cx={x(0)}
            cy={y(runs.find((run) => run.verdict === "kept")?.areaId ?? "")}
            r={3.5}
          >
            <title>Baseline</title>
          </circle>
        ) : null}
        {runs.map((run) => {
          const slot = experiment.areas[laneOf(run.areaId)]?.slot ?? 1
          const cx = x(run.number)
          const cy = y(run.areaId)
          if (run.verdict === "running")
            return (
              <g
                key={run.id}
                className="xp-live-dot"
                style={{ ["--xp-dot" as string]: `var(--xp-area-${slot})` }}
              >
                <circle cx={cx} cy={cy} r={10} className="xp-live-halo" />
                <circle cx={cx} cy={cy} r={4.5} className="xp-live-ring" />
              </g>
            )
          return (
            <circle
              key={run.id}
              className="xp-map-dot"
              data-verdict={run.verdict}
              data-hovered={hovered?.id === run.id || undefined}
              style={{ ["--xp-dot" as string]: `var(--xp-area-${slot})` }}
              cx={cx}
              cy={cy}
              r={run.verdict === "kept" ? 6 : 4.5}
            />
          )
        })}
        {runs.map((run) => (
          <circle
            key={`hit-${run.id}`}
            className="xp-hit"
            cx={x(run.number)}
            cy={y(run.areaId)}
            r={12}
            onPointerEnter={() => setHovered(run)}
            onPointerLeave={() =>
              setHovered((current) => (current?.id === run.id ? null : current))
            }
            onClick={() => onPick(run.id)}
          >
            <title>
              #{run.number} {run.title} — {verdictLabels[run.verdict]}
            </title>
          </circle>
        ))}
      </svg>
      {hovered ? (
        <ClimbTooltip
          experiment={experiment}
          run={hovered}
          left={x(hovered.number)}
          top={y(hovered.areaId)}
        />
      ) : null}
    </div>
  )
}

function AreaCard({
  experiment,
  areaId,
  onPick,
}: {
  experiment: Experiment
  areaId: string
  onPick: (runId: string) => void
}) {
  const area = experiment.areas.find((each) => each.id === areaId)
  if (!area) return null
  const summary = summarizeArea(experiment, area)
  const runs = experiment.runs.filter((run) => run.areaId === area.id)
  const latest = summary.latest
  return (
    <article
      className="xp-card xp-area-card"
      style={{ ["--xp-area" as string]: `var(--xp-area-${area.slot})` }}
    >
      <header className="xp-area-head">
        <span className="xp-area-name">
          <AreaMark area={area} size={15} />
          {area.name}
        </span>
        <AreaStatePill state={summary.state} />
      </header>
      <p className="xp-area-hypothesis">{area.hypothesis}</p>
      <div className="xp-area-stats">
        <div>
          <span className="xp-area-gain" data-empty={summary.gain === 0 || undefined}>
            {summary.gain === 0 ? "—" : points(summary.gain)}
          </span>
          <span className="xp-label">pts to best</span>
        </div>
        <div>
          <span className="xp-area-count">
            {summary.kept}
            <span className="xp-faint">/{summary.attempts}</span>
          </span>
          <span className="xp-label">kept</span>
        </div>
      </div>
      <div className="xp-strip" aria-label="Attempts in order">
        {runs.map((run) => (
          <button
            key={run.id}
            type="button"
            className="xp-strip-mark"
            data-verdict={run.verdict}
            onClick={() => onPick(run.id)}
            aria-label={`#${run.number} ${run.title}: ${verdictLabels[run.verdict]}`}
          />
        ))}
      </div>
      <footer className="xp-area-foot">
        <span className="xp-area-agents">
          {summary.agents.map((agent) => (
            <span key={agent.id} className="xp-area-agent">
              <AgentAvatar seed={agent.id} size={18} />
              {agent.name}
            </span>
          ))}
        </span>
        {latest ? (
          <button
            type="button"
            className="xp-area-latest xp-truncate"
            onClick={() => onPick(latest.id)}
          >
            {latest.verdict === "running" ? "Trying" : "Last"}: {latest.title}
            {latest.verdict === "running" && latest.progress
              ? ` · ${Math.round((latest.progress.done / latest.progress.total) * 100)}%`
              : ""}
          </button>
        ) : null}
      </footer>
    </article>
  )
}
