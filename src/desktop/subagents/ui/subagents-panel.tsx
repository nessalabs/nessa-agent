import { useLayoutEffect, useRef, useState } from "react"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import {
  useFocusedSubagent,
  useSendToSubagent,
  useSessionSubagents,
} from "../adapters/react/subagents-provider"
import { Composer } from "../../ui/composer"
import { ReadOnlyTranscript } from "../../workspace"
import {
  byActivity,
  stateCounts,
  stateLabels,
  type SessionSubagents,
  type Subagent,
  type SubagentTag,
  type SubagentState,
} from "../model/subagent"
import { characterLine } from "../model/character"
import { count, exact, running } from "../../ui/format"
import { tooltip } from "../../ui/tooltip"
import "./subagents.css"

/**
 * A conversation's subagents, in a panel of its own: every one of them and
 * what it is doing now, and — chosen from the list, or from anywhere in the
 * window that names one — one subagent in detail.
 */
export function SubagentsPanel({ id: sessionId }: { id: string }) {
  const swarm = useSessionSubagents(sessionId)
  const [focused, setFocused] = useFocusedSubagent(sessionId)
  if (!swarm)
    return (
      <section className="sa-panel">
        <p className="sa-empty">This conversation has no subagents.</p>
      </section>
    )
  const subagent = swarm.subagents.find((each) => each.id === focused)
  return (
    <section className="sa-panel" aria-label="Subagents">
      {subagent ? (
        <SubagentDetail
          key={subagent.id}
          swarm={swarm}
          subagent={subagent}
          onBack={() => setFocused(null)}
        />
      ) : (
        <SubagentList swarm={swarm} onPick={setFocused} />
      )}
    </section>
  )
}

function Summary({ swarm }: { swarm: SessionSubagents }) {
  const counts = stateCounts(swarm.subagents)
  const parts = (["working", "thinking", "stuck", "resting"] as const)
    .filter((state) => counts[state] > 0)
    .map((state) => `${count(counts[state])} ${stateLabels[state].toLowerCase()}`)
  return <p className="sa-summary">{parts.join(" · ")}</p>
}

/** Every subagent, those at work first, each saying what it is doing in a line. */
function SubagentList({
  swarm,
  onPick,
}: {
  swarm: SessionSubagents
  onPick: (subagentId: string) => void
}) {
  const ordered = byActivity(swarm.subagents)
  return (
    <>
      <header className="sa-head">
        <h2>Subagents</h2>
        <Summary swarm={swarm} />
      </header>
      <ul className="sa-list">
        {ordered.map((subagent) => (
          <li key={subagent.id}>
            <button type="button" className="sa-row" onClick={() => onPick(subagent.id)}>
              <span className="sa-row-avatar" {...tooltip(characterLine(subagent.seed))}>
                <Avatar subagent={subagent} size={30} />
              </span>
              <span className="sa-row-top">
                <span className="sa-name">{subagent.name}</span>
                <TagChip tag={tagOf(swarm, subagent)} />
                <span className="sa-time">{running(subagent.since, swarm.asOf)}</span>
              </span>
              <span className="sa-row-bottom">
                {subagent.work?.progress ? (
                  <>
                    <span className="sa-headline sa-work">{subagent.work.title}</span>
                    <Progress work={subagent.work.progress} compact />
                  </>
                ) : (
                  <>
                    <StateWord state={subagent.state} />
                    <span className="sa-headline">{subagent.headline}</span>
                  </>
                )}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </>
  )
}

/**
 * One subagent, as a conversation like any other: who it is, then its own
 * chat — the brief it was given and every turn since, with what it is doing
 * now — and a composer to say something to it. The pane's breadcrumb leads
 * back to the list, as Escape does.
 */
function SubagentDetail({
  swarm,
  subagent,
  onBack,
}: {
  swarm: SessionSubagents
  subagent: Subagent
  onBack: () => void
}) {
  const send = useSendToSubagent(swarm.sessionId)
  const [text, setText] = useState("")
  const [model, setModel] = useState(subagent.model)
  const scrollRef = useRef<HTMLDivElement>(null)
  // Opens at its latest turn, and follows new ones while read from the bottom.
  const messages = subagent.conversation.messages
  const atBottom = useRef(true)
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    if (scroller && atBottom.current) scroller.scrollTop = scroller.scrollHeight
  }, [messages.length, subagent.conversation.activity?.label])
  return (
    <div
      className="sa-chat"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.stopPropagation()
          onBack()
        }
      }}
    >
      <header className="sa-chat-head">
        <div className="sa-detail-head">
          <Avatar subagent={subagent} size={40} />
          <div>
            {/* Its name and the tag it was spun up with, together; what it is
                doing now is the chat's own live line. */}
            <div className="sa-name-line">
              <h2>{subagent.name}</h2>
              <TagChip tag={tagOf(swarm, subagent)} />
            </div>
            <p className="sa-character">{characterLine(subagent.seed)}</p>
          </div>
        </div>
      </header>
      <div
        ref={scrollRef}
        className="workspace-transcript sa-chat-scroll"
        onScroll={(event) => {
          const el = event.currentTarget
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40
        }}
      >
        <ReadOnlyTranscript
          sessionId={`${swarm.sessionId}:${subagent.id}`}
          messages={messages}
          activity={subagent.conversation.activity}
        />
      </div>
      <div className="workspace-dock sa-chat-dock">
        <Composer
          text={text}
          onTextChange={setText}
          page={false}
          onPageChange={() => {}}
          model={model}
          onModelChange={setModel}
          onSend={(said) => {
            send(subagent.id, said)
            setText("")
            atBottom.current = true
          }}
          placeholder={`Message ${subagent.name}…`}
        />
      </div>
    </div>
  )
}

function tagOf(swarm: SessionSubagents, subagent: Subagent): SubagentTag | undefined {
  return swarm.tags.find((tag) => tag.id === subagent.tagId)
}

function Avatar({ subagent, size }: { subagent: Subagent; size: number }) {
  return (
    <RandomAvatar
      seed={subagent.seed}
      name={subagent.name}
      ground="ink"
      busy={subagent.state === "working"}
      className="sa-avatar"
      style={{ width: size, height: size }}
    />
  )
}

function TagChip({ tag }: { tag: SubagentTag | undefined }) {
  if (!tag) return null
  return (
    <span
      className="sa-tag"
      style={{ ["--sa-hue" as string]: `var(--desktop-series-${tag.hue})` }}
    >
      <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden>
        <path d={tag.glyph} />
      </svg>
      <span className="sa-tag-name">{tag.name}</span>
    </span>
  )
}

function StateWord({ state }: { state: SubagentState }) {
  return (
    <span className="sa-state" data-state={state}>
      {stateLabels[state]}
    </span>
  )
}

function Progress({
  work,
  compact = false,
}: {
  work: { done: number; total: number }
  /** Beside a line of text: a short bar, no longer than it needs. */
  compact?: boolean
}) {
  const share = work.total === 0 ? 0 : work.done / work.total
  return (
    <span
      className="sa-progress"
      data-compact={compact || undefined}
      {...tooltip(`${exact(work.done)} of ${exact(work.total)}`)}
    >
      <span
        className="sa-meter"
        role="meter"
        aria-valuemin={0}
        aria-valuemax={1}
        aria-valuenow={share}
        aria-label="Progress"
      >
        <span style={{ transform: `scaleX(${share})` }} />
      </span>
      <span className="sa-progress-count">
        {count(work.done)} / {count(work.total)}
      </span>
    </span>
  )
}
