import { useId, useLayoutEffect, useMemo, useRef, useState } from "react"
import {
  bestSoFar,
  champion,
  settled,
  testDelta,
  trainDelta,
  verdictLabels,
  type Experiment,
  type Run,
} from "../model/experiment"
import { points, score } from "./format"
import { count } from "../../ui/format"
import { AreaMark, VerdictPill } from "./parts"
import { useWidth } from "./use-width"

const margin = { top: 22, right: 72, bottom: 28, left: 36 }

/**
 * The climb: the best test score after every run as a step line, the noise
 * band around it, and every run as a dot — kept ones solid, reverted ones
 * faint, overfit and costly ones ringed in amber, live ones pulsing where
 * they would land. Dots are drawn in ink, not by area: choosing an area
 * colours its dots and dims the rest, so the chart never asks the eye to
 * tell five hues apart at once.
 */
export function ClimbChart({
  experiment,
  highlight,
  onPick,
  height = 260,
}: {
  experiment: Experiment
  highlight: string | null
  onPick: (runId: string) => void
  height?: number
}) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const clip = useId()
  const wash = useId()
  const [hovered, setHovered] = useState<Run | null>(null)

  const geometry = useMemo(() => {
    const climb = bestSoFar(experiment)
    const done = experiment.runs.filter(settled)
    const live = experiment.runs.filter((run) => run.verdict === "running")
    const best = champion(experiment)
    const lastNumber = Math.max(
      ...experiment.runs
        .filter((run) => run.verdict !== "queued")
        .map((run) => run.number),
      1,
    )
    const scores = [
      ...done.map((run) => run.test.mean),
      experiment.baseline.test?.mean ?? 0,
      experiment.ceiling,
    ]
    const step = 5
    const lo = Math.floor((Math.min(...scores) - 2) / step) * step
    const hi = Math.ceil((Math.max(...scores) + 0.5) / step) * step
    const plotW = Math.max(width - margin.left - margin.right, 80)
    const plotH = height - margin.top - margin.bottom
    const x = (number: number) => margin.left + (number / (lastNumber + 0.6)) * plotW
    const y = (value: number) => margin.top + (1 - (value - lo) / (hi - lo)) * plotH
    const ticks: number[] = []
    for (let value = lo; value <= hi; value += hi - lo > 30 ? 10 : step) ticks.push(value)
    const xTicks: number[] = []
    const every = lastNumber > 40 ? 20 : 10
    for (let number = every; number <= lastNumber; number += every) xTicks.push(number)

    // Step-after through the climb, carried on to the newest run.
    const stepPath = (offset: number) => {
      let path = ""
      climb.forEach((point, index) => {
        const px = x(point.number)
        const py = y(point.best + offset)
        path += index === 0 ? `M${px},${py}` : `H${px}V${py}`
      })
      return `${path}H${x(lastNumber)}`
    }
    const upper = stepPath(experiment.noise)
    const lowerPoints = [...climb].reverse()
    let lower = `V${y(lowerPoints[0].best - experiment.noise)}`
    lowerPoints.forEach((point, index) => {
      lower += `H${x(point.number)}`
      const previous = lowerPoints[index + 1]
      if (previous) lower += `V${y(previous.best - experiment.noise)}`
    })
    const band = `${upper}${lower}Z`
    const line = stepPath(0)
    const under = `${line}V${margin.top + plotH}H${x(0)}Z`
    return {
      done,
      live,
      best,
      x,
      y,
      ticks,
      xTicks,
      band,
      line,
      under,
      plotW,
      plotH,
      lastNumber,
    }
  }, [experiment, width, height])

  const { done, live, best, x, y, ticks, xTicks, band, line, under, plotH, lastNumber } =
    geometry
  const baseline = experiment.baseline.test?.mean ?? 0
  const pickable = [...done, ...live]

  const nearest = (clientX: number, element: SVGSVGElement) => {
    const box = element.getBoundingClientRect()
    const at = clientX - box.left
    let closest: Run | null = null
    let distance = Infinity
    for (const run of pickable) {
      const d = Math.abs(x(run.number) - at)
      if (d < distance) {
        distance = d
        closest = run
      }
    }
    return distance < 24 ? closest : null
  }

  const hoveredY =
    hovered === null ? 0 : hovered.test ? y(hovered.test.mean) : y(best.test.mean)

  return (
    <div className="xp-climb" ref={ref} style={{ height }}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label={`Best test score by run: ${score(baseline)} at baseline, ${score(best.test.mean)} now`}
        onPointerMove={(event) => setHovered(nearest(event.clientX, event.currentTarget))}
        onPointerLeave={() => setHovered(null)}
        onClick={(event) => {
          const run = nearest(event.clientX, event.currentTarget)
          if (run) onPick(run.id)
        }}
        data-hovering={hovered !== null || undefined}
      >
        <defs>
          <clipPath id={clip}>
            <rect
              x={margin.left}
              y={margin.top}
              width={Math.max(width - margin.left - margin.right, 0)}
              height={plotH}
            />
          </clipPath>
          <linearGradient id={wash} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0" stopColor="var(--xp-ink)" stopOpacity="0.14" />
            <stop offset="1" stopColor="var(--xp-ink)" stopOpacity="0" />
          </linearGradient>
        </defs>

        {ticks.map((value) => (
          <g key={value} className="xp-axis">
            <line
              x1={margin.left}
              x2={width - margin.right}
              y1={y(value)}
              y2={y(value)}
            />
            <text x={margin.left - 10} y={y(value)} dy="0.32em" textAnchor="end">
              {value}
            </text>
          </g>
        ))}
        {xTicks.map((number) => (
          <text
            key={number}
            className="xp-axis-x"
            x={x(number)}
            y={height - 8}
            textAnchor="middle"
          >
            #{number}
          </text>
        ))}

        <g clipPath={`url(#${clip})`}>
          <path className="xp-climb-band" d={band} />
          <path d={under} fill={`url(#${wash})`} />
        </g>

        <line
          className="xp-reference"
          x1={margin.left}
          x2={width - margin.right}
          y1={y(experiment.ceiling)}
          y2={y(experiment.ceiling)}
        />
        <text
          className="xp-reference-label"
          x={margin.left + 6}
          y={y(experiment.ceiling) - 6}
        >
          Ceiling {score(experiment.ceiling)} · best model, max effort
        </text>
        <line
          className="xp-reference"
          x1={margin.left}
          x2={width - margin.right}
          y1={y(baseline)}
          y2={y(baseline)}
        />
        <text
          className="xp-reference-label"
          x={width - margin.right - 6}
          y={y(baseline) + 14}
          textAnchor="end"
        >
          Baseline {score(baseline)}
        </text>

        <path className="xp-climb-line" d={line} />

        {hovered ? (
          <line
            className="xp-crosshair"
            x1={x(hovered.number)}
            x2={x(hovered.number)}
            y1={margin.top}
            y2={margin.top + plotH}
          />
        ) : null}

        {done.map((run) => {
          const dimmed = highlight !== null && run.areaId !== highlight
          const lit = highlight !== null && run.areaId === highlight
          const slot = experiment.areas.find((area) => area.id === run.areaId)?.slot ?? 1
          return (
            <circle
              key={run.id}
              className="xp-dot"
              data-verdict={run.verdict}
              data-dimmed={dimmed || undefined}
              data-lit={lit || undefined}
              data-hovered={hovered?.id === run.id || undefined}
              style={
                lit ? { ["--xp-dot" as string]: `var(--xp-area-${slot})` } : undefined
              }
              cx={x(run.number)}
              cy={y(run.test.mean)}
              r={
                run.verdict === "kept"
                  ? 5
                  : run.verdict === "overfit" || run.verdict === "costly"
                    ? 4
                    : 3.5
              }
            />
          )
        })}

        {live.map((run) => (
          <g
            key={run.id}
            className="xp-live-dot"
            data-dimmed={(highlight !== null && run.areaId !== highlight) || undefined}
          >
            <circle
              cx={x(run.number)}
              cy={y(best.test.mean)}
              r={10}
              className="xp-live-halo"
            />
            <circle
              cx={x(run.number)}
              cy={y(best.test.mean)}
              r={4.5}
              className="xp-live-ring"
            />
          </g>
        ))}

        <g className="xp-end">
          <circle cx={x(lastNumber)} cy={y(best.test.mean)} r={3} />
          <text x={x(lastNumber) + 10} y={y(best.test.mean)} dy="0.32em">
            {score(best.test.mean)}
          </text>
          <text
            className="xp-end-caption"
            x={x(lastNumber) + 10}
            y={y(best.test.mean) + 15}
          >
            best
          </text>
        </g>
      </svg>
      {hovered ? (
        <ClimbTooltip
          experiment={experiment}
          run={hovered}
          left={x(hovered.number)}
          top={hoveredY}
        />
      ) : null}
    </div>
  )
}

/**
 * A run's card beside the point it describes. It measures itself against what
 * is visible — the experiment's scroll area, and the window — and takes the
 * side of the point that fits, sliding along the other axis to stay whole, so
 * a point at any edge still shows all of it.
 */
export function ClimbTooltip({
  experiment,
  run,
  left,
  top,
}: {
  experiment: Experiment
  run: Run
  /** The point, in the chart's own coordinates. */
  left: number
  top: number
}) {
  const area = experiment.areas.find((each) => each.id === run.areaId)
  const agent = experiment.agents.find((each) => each.id === run.agentId)
  const test = testDelta(experiment, run)
  const train = trainDelta(experiment, run)
  const ref = useRef<HTMLDivElement>(null)
  const [place, setPlace] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const tip = ref.current
    const chart = tip?.offsetParent
    if (!tip || !chart) return
    const origin = chart.getBoundingClientRect()
    const area = tip.closest(".xp-scroll")?.getBoundingClientRect()
    const margin = 8
    const bounds = {
      left: Math.max(area?.left ?? 0, 0) + margin,
      right: Math.min(area?.right ?? window.innerWidth, window.innerWidth) - margin,
      top: Math.max(area?.top ?? 0, 0) + margin,
      bottom: Math.min(area?.bottom ?? window.innerHeight, window.innerHeight) - margin,
    }
    const { width, height } = tip.getBoundingClientRect()
    const gap = 14
    const pointX = origin.left + left
    const pointY = origin.top + top
    // The right of the point, else its left, else wherever it fits best.
    let x = pointX + gap
    if (x + width > bounds.right) x = pointX - gap - width
    x = Math.min(Math.max(x, bounds.left), Math.max(bounds.right - width, bounds.left))
    let y = pointY - 40
    y = Math.min(Math.max(y, bounds.top), Math.max(bounds.bottom - height, bounds.top))
    setPlace({ left: x - origin.left, top: y - origin.top })
  }, [left, top, run.id])
  return (
    <div
      ref={ref}
      className="xp-tooltip"
      role="tooltip"
      style={{
        left: place?.left ?? left + 14,
        top: place?.top ?? top - 40,
        visibility: place ? undefined : "hidden",
      }}
    >
      <div className="xp-tooltip-head">
        <span className="xp-mono">#{run.number}</span>
        <VerdictPill verdict={run.verdict} />
      </div>
      <p className="xp-tooltip-title">{run.title}</p>
      <div className="xp-tooltip-meta">
        <AreaMark area={area} size={12} />
        <span>{area?.name}</span>
        <span className="xp-sep">·</span>
        <span>{agent?.name}</span>
      </div>
      {run.test && run.train ? (
        <dl className="xp-tooltip-scores">
          <div>
            <dt>Test</dt>
            <dd>
              {score(run.test.mean)} <Delta value={test} noise={experiment.noise} />
            </dd>
          </div>
          <div>
            <dt>Train</dt>
            <dd>
              {score(run.train.mean)} <Delta value={train} noise={experiment.noise} />
            </dd>
          </div>
        </dl>
      ) : run.progress ? (
        <p className="xp-tooltip-progress">
          {verdictLabels.running} · {count(run.progress.done)} of{" "}
          {count(run.progress.total)} cases
        </p>
      ) : null}
    </div>
  )
}

/** A signed change, toned only when it clears the noise. */
export function Delta({ value, noise }: { value: number | undefined; noise: number }) {
  if (value === undefined) return null
  const tone = value > noise ? "up" : value < -noise ? "down" : "flat"
  return (
    <span className="xp-delta" data-tone={tone}>
      {points(value)}
    </span>
  )
}
