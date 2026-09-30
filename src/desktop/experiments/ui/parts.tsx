import type { ReactNode } from "react"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import { areaGlyphPath } from "./area-glyphs"
import { tooltip } from "../../ui/tooltip"
import {
  areaStateLabels,
  verdictLabels,
  type Agent,
  type Area,
  type AreaState,
  type NoteTone,
  type Verdict,
} from "../model/experiment"

/*
 * Marks that say what something is rather than only which colour it is: an
 * area is drawn as what it works on, a verdict as what happened. Each is a
 * line drawing on a 16-unit grid in `currentColor`, so it takes its tint
 * from wherever it sits.
 */

/** An area's glyph as SVG children, for drawing inside a chart. */
export function areaGlyph(area: Area | undefined): ReactNode {
  return <path d={areaGlyphPath(area?.id)} />
}

/** An area's mark beside its name: what it works on, in its hue. */
export function AreaMark({ area, size = 13 }: { area: Area | undefined; size?: number }) {
  return (
    <svg
      className="xp-area-mark"
      aria-hidden
      width={size}
      height={size}
      viewBox="0 0 16 16"
      style={{ color: `var(--xp-area-${area?.slot ?? 1})` }}
    >
      {areaGlyph(area)}
    </svg>
  )
}

const verdictGlyphs: Record<Verdict, ReactNode> = {
  kept: <path d="M2.8 6.4l2.2 2.2 4.4-5" />,
  // Fitted the train cases: a wave that does not carry through.
  overfit: <path d="M1.5 7.2c1.3-2.6 2.8-2.6 4 0s2.8 2.6 4 0" />,
  regressed: <path d="M6 2.5v7M3.2 6.8L6 9.6l2.8-2.8" />,
  flat: <path d="M3 6h6" />,
  // Cost went up.
  costly: <path d="M2 9.2l2.8-2.8 1.8 1.8L10 4.8M7.6 4.8H10v2.4" />,
  running: <path d="M6 1.8a4.2 4.2 0 1 1-4.2 4.2" />,
  queued: (
    <>
      <circle cx="6" cy="6" r="4.2" />
      <path d="M6 3.8V6l1.5 1" />
    </>
  ),
}

/** A verdict as a word beside a small drawing of what happened. */
export function VerdictPill({ verdict }: { verdict: Verdict }) {
  return (
    <span
      className="xp-verdict"
      data-verdict={verdict}
      {...tooltip(verdictLabels[verdict])}
    >
      <svg
        aria-hidden
        className="xp-verdict-mark"
        width="12"
        height="12"
        viewBox="0 0 12 12"
      >
        {verdictGlyphs[verdict]}
      </svg>
      <span className="xp-verdict-word">{verdictLabels[verdict]}</span>
    </span>
  )
}

const stateGlyphs: Record<AreaState, ReactNode> = {
  climbing: <path d="M2 9l3-3 2 2 3-3.5M7.8 4.5H10v2.2" />,
  stalling: <path d="M2 6.5h5.5M9.5 3.5v6" />,
  exhausted: (
    <>
      <circle cx="6" cy="6" r="4" />
      <path d="M3.2 8.8l5.6-5.6" />
    </>
  ),
  early: <path d="M6 10V5.5M6 7.5c0-2 1.6-3.2 3.5-3.2M6 6.5C6 5 4.6 4 3 4" />,
}

export function AreaStatePill({ state }: { state: AreaState }) {
  return (
    <span className="xp-state" data-state={state}>
      <svg
        aria-hidden
        className="xp-verdict-mark"
        width="12"
        height="12"
        viewBox="0 0 12 12"
      >
        {stateGlyphs[state]}
      </svg>
      {areaStateLabels[state]}
    </span>
  )
}

const toneGlyphs: Record<NoteTone, ReactNode> = {
  good: <path d="M6 9.5v-7M3.2 5.2L6 2.4l2.8 2.8" />,
  warning: <path d="M6 2.5v4.5M6 9.2v.3" />,
  info: <path d="M6 5.5v4M6 2.8v.3" />,
}

/** What kind of thing the hill-climber noticed. */
export function NoteMark({ tone }: { tone: NoteTone }) {
  return (
    <svg
      className="xp-note-mark"
      data-tone={tone}
      aria-label={tone}
      role="img"
      width="12"
      height="12"
      viewBox="0 0 12 12"
    >
      {toneGlyphs[tone]}
    </svg>
  )
}

/** An agent's generative mark; it breathes while the agent works. */
export function AgentAvatar({
  seed,
  name,
  working = false,
  size = 24,
}: {
  seed: string
  name?: string
  working?: boolean
  size?: number
}) {
  return (
    <RandomAvatar
      seed={`xp-agent-${seed}`}
      name={name}
      ground="ink"
      busy={working}
      className="xp-avatar"
      style={{ width: size, height: size }}
    />
  )
}

/**
 * The swarm in a glance: the three agents doing the most, overlapped, and how
 * many more are working beside them.
 */
export function Swarmpile({
  agents,
  size = 22,
}: {
  agents: readonly Agent[]
  size?: number
}) {
  const order: Record<Agent["activity"]["kind"], number> = {
    evaluating: 0,
    drafting: 1,
    diagnosing: 2,
    resting: 3,
  }
  const shown = [...agents]
    .sort((a, b) => order[a.activity.kind] - order[b.activity.kind])
    .slice(0, 3)
  const more = agents.length - shown.length
  return (
    <span className="xp-swarmpile" aria-label={`${agents.length} agents working`}>
      <span className="xp-facepile">
        {shown.map((agent) => (
          <AgentAvatar key={agent.id} seed={agent.id} name={agent.name} size={size} />
        ))}
      </span>
      {more > 0 ? <span className="xp-swarmpile-more">+{more}</span> : null}
    </span>
  )
}

/** A thin track filling left to right. */
export function Meter({ value, label }: { value: number; label: string }) {
  return (
    <span
      className="xp-meter"
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={1}
      aria-valuenow={value}
    >
      <span style={{ transform: `scaleX(${Math.max(0, Math.min(1, value))})` }} />
    </span>
  )
}
