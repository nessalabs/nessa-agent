import { useState } from "react"
import {
  champion,
  pathToBest,
  runById,
  totals,
  type Agent,
  type Experiment,
} from "../model/experiment"
import { AreaMenu } from "./area-menu"
import { ClimbChart } from "./climb-chart"
import { dollars, percentChange, points, score } from "./format"
import { ago, count, running } from "../../ui/format"
import { AgentAvatar, AreaMark, Meter, NoteMark } from "./parts"

/**
 * The experiment at a glance: how good it is now and how far it came, the
 * climb that got it there, the changes that make up the best version, what
 * every agent is doing this minute, and what the hill-climber noticed.
 */
export function Overview({
  experiment,
  onPick,
  onOpenAgent,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
  /** Opens an agent in its conversation's subagents panel, where it has one. */
  onOpenAgent?: (agentId: string) => void
}) {
  const [highlight, setHighlight] = useState<string | null>(null)
  return (
    <div className="xp-overview">
      <Headline experiment={experiment} />
      <section className="xp-card xp-card-flush" aria-labelledby="xp-climb-title">
        <header className="xp-card-head">
          <div>
            <h3 id="xp-climb-title">The climb</h3>
            <p>
              Best test score after each run, inside the ±{experiment.noise.toFixed(1)}{" "}
              noise band
            </p>
          </div>
          <AreaMenu
            experiment={experiment}
            value={highlight}
            onChange={setHighlight}
            heading="Highlight on the climb"
            allLabel="Every area"
          />
        </header>
        <ClimbChart experiment={experiment} highlight={highlight} onPick={onPick} />
        <ClimbLegend />
      </section>
      <div className="xp-columns">
        <PathToBest experiment={experiment} onPick={onPick} />
        <Swarm experiment={experiment} onPick={onPick} onOpenAgent={onOpenAgent} />
      </div>
      <Notes experiment={experiment} onPick={onPick} />
    </div>
  )
}

function Headline({ experiment }: { experiment: Experiment }) {
  const best = champion(experiment)
  const baseline = experiment.baseline
  const count = totals(experiment)
  const gain = best.test.mean - (baseline.test?.mean ?? 0)
  const headroom = experiment.ceiling - (baseline.test?.mean ?? 0)
  const closed = headroom > 0 ? gain / headroom : 0
  return (
    <section className="xp-headline" aria-label="Where the experiment stands">
      <div className="xp-hero">
        <span className="xp-label">Best so far · test</span>
        <div className="xp-hero-figure">
          <span className="xp-hero-value">{score(best.test.mean)}</span>
          <span className="xp-hero-unit">%</span>
          <span className="xp-hero-delta" data-tone="up">
            {points(gain)} pts
          </span>
        </div>
        <p className="xp-hero-caption">
          from {score(baseline.test?.mean ?? 0)} baseline · closed{" "}
          <strong>{Math.round(closed * 100)}%</strong> of the gap to the{" "}
          {score(experiment.ceiling)} ceiling
        </p>
        <div className="xp-gap" aria-hidden>
          <span className="xp-gap-track" />
          <span className="xp-gap-fill" style={{ width: `${closed * 100}%` }} />
        </div>
      </div>
      <dl className="xp-tiles">
        <div className="xp-tile">
          <dt>Runs</dt>
          <dd>
            {count.spent}
            <span className="xp-tile-of"> / {experiment.budget}</span>
          </dd>
          <Meter value={count.spent / experiment.budget} label="Budget spent" />
        </div>
        <div className="xp-tile">
          <dt>Kept</dt>
          <dd>{count.kept}</dd>
          <span className="xp-tile-note">
            {count.reverted} reverted · {count.overfit} overfit
          </span>
        </div>
        <div className="xp-tile">
          <dt>Cost per task</dt>
          <dd>{dollars(best.cost ?? 0)}</dd>
          <span className="xp-tile-note">
            {percentChange(baseline.cost ?? 1, best.cost ?? 1)} vs baseline
          </span>
        </div>
        <div className="xp-tile">
          <dt>Running</dt>
          <dd>{ago(experiment.startedAt, experiment.asOf)}</dd>
          <span className="xp-tile-note">
            {count.live} evaluating · {count.queued} queued
          </span>
        </div>
      </dl>
    </section>
  )
}

function ClimbLegend() {
  return (
    <ul className="xp-legend" aria-label="Legend">
      <li>
        <svg width="14" height="10" aria-hidden>
          <path d="M0,8H6V2H14" className="xp-legend-line" />
        </svg>
        Best so far
      </li>
      <li>
        <span className="xp-legend-dot" data-verdict="kept" />
        Kept
      </li>
      <li>
        <span className="xp-legend-dot" data-verdict="flat" />
        Reverted
      </li>
      <li>
        <span className="xp-legend-dot" data-verdict="overfit" />
        Overfit or too costly
      </li>
      <li>
        <span className="xp-legend-dot" data-verdict="running" />
        Evaluating
      </li>
      <li>
        <span className="xp-legend-band" />
        Noise
      </li>
    </ul>
  )
}

function PathToBest({
  experiment,
  onPick,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
}) {
  const steps = pathToBest(experiment)
  const baseline = experiment.baseline.test?.mean ?? 0
  const best = champion(experiment).test.mean
  const span = Math.max(best - baseline, 0.1)
  let level = baseline
  return (
    <section className="xp-card" aria-labelledby="xp-path-title">
      <header className="xp-card-head">
        <div>
          <h3 id="xp-path-title">Path to the best version</h3>
          <p>{steps.length} kept changes, stacked on the baseline</p>
        </div>
      </header>
      <ol className="xp-path">
        <li className="xp-path-end">
          <span className="xp-path-label">Baseline</span>
          <span className="xp-path-value">{score(baseline)}</span>
        </li>
        {steps.map((step) => {
          const area = experiment.areas.find((each) => each.id === step.run.areaId)
          const agent = experiment.agents.find((each) => each.id === step.run.agentId)
          const left = ((level - baseline) / span) * 100
          const width = (step.gain / span) * 100
          level += step.gain
          return (
            <li key={step.run.id}>
              <button
                type="button"
                className="xp-path-row"
                onClick={() => onPick(step.run.id)}
              >
                <span className="xp-path-title">
                  <AreaMark area={area} />
                  <span className="xp-truncate">{step.run.title}</span>
                </span>
                <span className="xp-path-meta">
                  #{step.run.number} · {agent?.name}
                </span>
                <span className="xp-path-track" aria-hidden>
                  <span
                    style={{
                      left: `${left}%`,
                      width: `${Math.max(width, 1.5)}%`,
                      background: `var(--xp-area-${area?.slot ?? 1})`,
                    }}
                  />
                </span>
                <span className="xp-path-gain">{points(step.gain)}</span>
              </button>
            </li>
          )
        })}
        <li className="xp-path-end" data-best>
          <span className="xp-path-label">Best so far</span>
          <span className="xp-path-value">{score(best)}</span>
        </li>
      </ol>
    </section>
  )
}

const activityOrder: Record<Agent["activity"]["kind"], number> = {
  evaluating: 0,
  drafting: 1,
  diagnosing: 2,
  resting: 3,
}

function Swarm({
  experiment,
  onPick,
  onOpenAgent,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
  onOpenAgent?: (agentId: string) => void
}) {
  const agents = [...experiment.agents].sort((a, b) => {
    const order = activityOrder[a.activity.kind] - activityOrder[b.activity.kind]
    if (order !== 0) return order
    const progress = (agent: Agent) => {
      if (agent.activity.kind !== "evaluating") return 0
      const run = runById(experiment, agent.activity.runId)
      return run?.progress ? run.progress.done / run.progress.total : 0
    }
    return progress(b) - progress(a)
  })
  const live = experiment.agents.filter(
    (agent) => agent.activity.kind === "evaluating",
  ).length
  return (
    <section className="xp-card" aria-labelledby="xp-swarm-title">
      <header className="xp-card-head">
        <div>
          <h3 id="xp-swarm-title">The swarm, now</h3>
          <p>
            {experiment.agents.length} agents · {live} evaluating
          </p>
        </div>
        <span className="xp-live" aria-hidden>
          <span className="xp-live-pulse" />
          Live
        </span>
      </header>
      <ul className="xp-swarm">
        {agents.map((agent) => (
          <SwarmRow
            key={agent.id}
            experiment={experiment}
            agent={agent}
            onPick={onPick}
            onOpenAgent={onOpenAgent}
          />
        ))}
      </ul>
    </section>
  )
}

const activityWords: Record<Agent["activity"]["kind"], string> = {
  evaluating: "Evaluating",
  drafting: "Drafting",
  diagnosing: "Diagnosing",
  resting: "Resting",
}

/**
 * One agent, in two lines at any width: who, then what — its evaluation's
 * progress, or what it is doing instead. It opens the agent in its
 * conversation's subagents panel, or, where there is none, its evaluation.
 */
function SwarmRow({
  experiment,
  agent,
  onPick,
  onOpenAgent,
}: {
  experiment: Experiment
  agent: Agent
  onPick: (runId: string) => void
  onOpenAgent?: (agentId: string) => void
}) {
  const area = experiment.areas.find((each) => each.id === agent.areaId)
  const activity = agent.activity
  const run =
    activity.kind === "evaluating" ? runById(experiment, activity.runId) : undefined
  const open = onOpenAgent
    ? () => onOpenAgent(agent.id)
    : run
      ? () => onPick(run.id)
      : undefined
  const content = (
    <>
      <AgentAvatar seed={agent.id} working={activity.kind === "evaluating"} size={30} />
      <span className="xp-swarm-top">
        <span className="xp-swarm-name">{agent.name}</span>
        <span className="xp-swarm-area">
          <AreaMark area={area} size={11} />
          <span className="xp-truncate">{area?.name}</span>
        </span>
        <span className="xp-swarm-time">{running(agent.since, experiment.asOf)}</span>
      </span>
      <span className="xp-swarm-bottom" data-kind={activity.kind}>
        {run?.progress ? (
          <>
            <span className="xp-swarm-task xp-truncate">
              <span className="xp-faint">#{run.number}</span> {run.title}
            </span>
            <span className="xp-swarm-progress">
              <Meter
                value={run.progress.done / run.progress.total}
                label={`${agent.name}'s evaluation`}
              />
              <span>
                {count(run.progress.done)}/{count(run.progress.total)}
              </span>
            </span>
          </>
        ) : (
          <>
            <span className="xp-swarm-flag">{activityWords[activity.kind]}</span>
            <span className="xp-truncate">
              {activity.kind === "evaluating" ? "" : activity.note}
            </span>
          </>
        )}
      </span>
    </>
  )
  return (
    <li>
      {open ? (
        <button type="button" className="xp-swarm-row" onClick={open}>
          {content}
        </button>
      ) : (
        <div className="xp-swarm-row">{content}</div>
      )}
    </li>
  )
}

function Notes({
  experiment,
  onPick,
}: {
  experiment: Experiment
  onPick: (runId: string) => void
}) {
  const notes = [...experiment.notes].reverse()
  return (
    <section className="xp-card" aria-labelledby="xp-notes-title">
      <header className="xp-card-head">
        <div>
          <h3 id="xp-notes-title">What the hill-climber noticed</h3>
          <p>Stalls, overfitting and new bests, newest first</p>
        </div>
      </header>
      <ul className="xp-notes">
        {notes.map((note) => (
          <li key={note.id} data-tone={note.tone}>
            <NoteMark tone={note.tone} />
            <p>{note.text}</p>
            <span className="xp-note-side">
              {note.runId ? (
                <button
                  type="button"
                  className="xp-link"
                  onClick={() => onPick(note.runId ?? "")}
                >
                  View run
                </button>
              ) : null}
              <span className="xp-mono">{ago(note.at, experiment.asOf)}</span>
            </span>
          </li>
        ))}
      </ul>
    </section>
  )
}
