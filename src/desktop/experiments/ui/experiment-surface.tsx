import { useLayoutEffect, useRef, useState } from "react"
import { useExperiment } from "../adapters/react/experiments-provider"
import { useFocusSubagent } from "../../subagents"
import { runById, type Experiment } from "../model/experiment"
import { Areas } from "./areas"
import { Selector } from "./selector"
import { Overview } from "./overview"
import { ago, count } from "../../ui/format"
import { Swarmpile } from "./parts"
import { LiveStatus } from "./live-status"
import { RunDetail } from "./run-detail"
import { Runs } from "./runs"
import "./experiments.css"

type View = "overview" | "areas" | "runs"

/**
 * An experiment opened beside its conversation or in a pane of its own: the
 * overview, the areas the swarm is exploring, and every run — any run
 * opening to fill the experiment, over whichever view it was picked from.
 */
export function ExperimentSurface({
  id,
  placement,
  wide = false,
  onToggleWide,
  onClose,
  onOpenWidget,
}: {
  id: string
  /** Beside its conversation, with its own close; or a pane of its own, whose header closes it. */
  placement: "beside" | "pane"
  wide?: boolean
  onToggleWide?: () => void
  onClose?: () => void
  /** Opens another widget in a pane of its own: its conversation's subagents. */
  onOpenWidget?: (widget: { plugin: string; id: string }) => void
}) {
  const experiment = useExperiment(id)
  const focusSubagent = useFocusSubagent()
  const [view, setView] = useState<View>("overview")
  // The runs opened, the one shown last: following a run's lineage adds to
  // the trail, and the breadcrumb walks it, so a detour never loses the run
  // it started from.
  const [trail, setTrail] = useState<readonly string[]>([])
  const picked = trail.at(-1) ?? null
  const open = (runId: string) => setTrail([runId])
  const follow = (runId: string) =>
    setTrail((current) => (current.at(-1) === runId ? current : [...current, runId]))
  const back = () => setTrail((current) => current.slice(0, -1))
  const close = () => setTrail([])
  const scrollRef = useRef<HTMLDivElement>(null)
  // Each view opens at its top, not where the last one was scrolled to.
  useLayoutEffect(() => {
    scrollRef.current?.scrollTo(0, 0)
  }, [view])
  if (!experiment) return null
  const sessionId = experiment.sessionId
  const openAgent =
    onOpenWidget && sessionId !== undefined
      ? (agentId: string) => {
          focusSubagent(sessionId, agentId)
          onOpenWidget({ plugin: "subagents", id: sessionId })
        }
      : undefined
  const views: Record<View, string> = {
    overview: "Overview",
    areas: "Areas",
    runs: "Runs",
  }
  const crumbs = [
    { label: views[view], onSelect: close },
    ...trail.slice(0, -1).map((runId, index) => {
      const run = runById(experiment, runId)
      return {
        label: run?.number === 0 ? "Baseline" : `Run ${run?.number ?? "?"}`,
        onSelect: () => setTrail((current) => current.slice(0, index + 1)),
      }
    }),
  ]
  return (
    <section
      className="xp-surface"
      data-placement={placement}
      aria-label={experiment.title}
      onKeyDown={(event) => {
        if (event.key === "Escape" && picked === null) onClose?.()
      }}
    >
      {/* Under an open run the view stays as it was — its tab, its scroll —
          hidden rather than unmounted, so stepping back finds it unchanged. */}
      <div className="xp-main" data-covered={picked !== null || undefined}>
        <SurfaceHeader
          experiment={experiment}
          wide={wide}
          onToggleWide={onToggleWide}
          onClose={onClose}
        />
        <nav className="xp-tabs">
          <Selector
            value={view}
            onChange={setView}
            label="Experiment view"
            options={[
              { value: "overview", label: "Overview" },
              { value: "areas", label: "Areas" },
              { value: "runs", label: "Runs" },
            ]}
          />
          <span className="xp-suite">
            {experiment.suite.name} · {count(experiment.suite.train)} train /{" "}
            {count(experiment.suite.test)} test
          </span>
        </nav>
        <div className="xp-scroll" ref={scrollRef}>
          <div className="xp-view" key={view}>
            {view === "overview" ? (
              <Overview experiment={experiment} onPick={open} onOpenAgent={openAgent} />
            ) : view === "areas" ? (
              <Areas experiment={experiment} onPick={open} />
            ) : (
              <Runs experiment={experiment} selected={picked} onPick={open} />
            )}
          </div>
        </div>
      </div>
      {picked !== null ? (
        <RunDetail
          experiment={experiment}
          runId={picked}
          crumbs={crumbs}
          onFollow={follow}
          onBack={back}
          onOpenAgent={openAgent}
        />
      ) : null}
    </section>
  )
}

function SurfaceHeader({
  experiment,
  wide,
  onToggleWide,
  onClose,
}: {
  experiment: Experiment
  wide: boolean
  onToggleWide?: () => void
  onClose?: () => void
}) {
  return (
    <header className="xp-surface-head">
      <div className="xp-surface-title">
        {/* A title's "·" stays with the words before it, so a wrap never starts a line with it. */}
        <h2>{experiment.title.replaceAll(" · ", "\u00a0· ")}</h2>
        <p className="xp-goal">{experiment.goal}</p>
        <div className="xp-swarm-line">
          <LiveStatus experiment={experiment} />
          <Swarmpile agents={experiment.agents} size={18} />
          <span>Running {ago(experiment.startedAt, experiment.asOf)}</span>
        </div>
      </div>
      <div className="xp-surface-actions">
        {onToggleWide ? (
          <button
            type="button"
            className="xp-icon-button"
            aria-label={
              wide
                ? "Show the conversation beside it"
                : "Give the experiment the whole pane"
            }
            aria-pressed={wide}
            onClick={onToggleWide}
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 14 14"
              aria-hidden
              fill="none"
              stroke="currentColor"
              strokeWidth="1.3"
            >
              {wide ? (
                <path d="M5 1.5v11M1.5 2.5h11v9h-11z" />
              ) : (
                <path d="M5 2H2v3M9 2h3v3M5 12H2V9M9 12h3V9" />
              )}
            </svg>
          </button>
        ) : null}
        {onClose ? (
          <button
            type="button"
            className="xp-icon-button"
            aria-label="Close experiment"
            onClick={onClose}
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 12 12"
              aria-hidden
              stroke="currentColor"
              strokeWidth="1.4"
            >
              <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
            </svg>
          </button>
        ) : null}
      </div>
    </header>
  )
}
