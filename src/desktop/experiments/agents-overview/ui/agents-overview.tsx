import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../../workspace"
import { openSession } from "../../../workspace"
import { matchesChord } from "../../../workspace/adapters/dom/shortcuts"
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import type { SessionSummary } from "../../../workspace/model/workspace-index"
import type { Approval } from "../../../workspace/model/transcript"
import { reducedMotion } from "../../../adapters/motion-preference"
import { durationOf, useReflow } from "../adapters/reflow"
import { useAtLeastWide } from "../adapters/surface-width"
import {
  answerRequest,
  selectGlance,
  selectReady,
  tagsOf,
} from "../adapters/workspace-bridge"
import { useAgentsFilterPreference } from "../adapters/preference"
import { useNow } from "../../../workspace/adapters/dom/clock"
import { FilterMenu } from "./filter-menu"
import {
  glanceLine,
  readingOrder,
  sameGlance,
  type AgentsGlance,
  type Held,
} from "../model/agents-glance"
import { answeredLabels, type Answer, type Settling } from "../model/request"
import { afterAnswer, stepFrom, type Step } from "../model/walk"
import { AllClear } from "./all-clear"
import { overviewKeys } from "./overview-keys"
import { RequestRow } from "./request-row"
import { SessionPeek } from "./session-peek"
import { SessionRow } from "./session-row"

/** How long an answered request says what became of it before it goes. */
const settledFor = 700

const steps: Partial<Record<string, Step>> = {
  next: "next",
  previous: "previous",
  first: "first",
  last: "last",
}

/**
 * Every agent at a glance, in the chat area: what waits on the person, with
 * each approval answerable where it stands; what is working; what finished
 * unseen — and a peek at the chosen one, beside the list where there is room
 * and beneath its row where there is not, to check in on it without opening
 * it. The keyboard walks the list with the arrows (the peek follows), opens
 * with ↩ and answers with ⌘↩, ⌥⌘↩ and ⌘⌫, moving on to the next request as
 * each one settles and leaves.
 */
export function AgentsOverview({ onLeave }: { onLeave: () => void }) {
  const dispatch = useWorkspaceDispatch()
  const ready = useWorkspaceSelector(selectReady)
  const [settling, setSettling] = useState<readonly Settling[]>([])
  const [failures, setFailures] = useState<ReadonlyMap<string, AnswerFailure>>(new Map())
  const [said, setSaid] = useState("")
  const held = useMemo<readonly Held[]>(
    () => settling.map(({ sessionId, updatedAt }) => ({ sessionId, updatedAt })),
    [settling],
  )
  const [filter, setFilter] = useAgentsFilterPreference()
  const now = useNow(60_000)
  const [active, setActive] = useState<string | null>(null)
  const glance = useWorkspaceSelector(
    (state) => selectGlance(state, held, { filter, now, tagsOf, looking: active }),
    sameGlance,
  )
  const order = useMemo(() => readingOrder(glance), [glance])
  const current = active !== null && order.includes(active) ? active : (order[0] ?? null)
  const list = useRef<HTMLDivElement>(null)
  useReflow(list)

  const focusItem = useCallback((id: string | null) => {
    const column = list.current
    if (!column) return
    if (id === null) {
      // Nothing left to answer: back to the top, where the list says so.
      column.focus({ preventScroll: true })
      column.closest(".agents-overview-scroll")?.scrollTo({
        top: 0,
        behavior: reducedMotion() ? "auto" : "smooth",
      })
      return
    }
    setActive(id)
    const item = column.querySelector<HTMLElement>(
      `[data-overview-item="${CSS.escape(id)}"]`,
    )
    item?.focus({ preventScroll: true })
    item?.scrollIntoView({
      block: "nearest",
      behavior: reducedMotion() ? "auto" : "smooth",
    })
  }, [])

  // Opened, the keyboard is here, on the first thing listed.
  const arrived = useRef(false)
  useEffect(() => {
    if (arrived.current || !ready) return
    arrived.current = true
    focusItem(readingOrder(glance)[0] ?? null)
  }, [ready, glance, focusItem])

  const timers = useRef(new Set<number>())
  useEffect(() => {
    const pending = timers.current
    return () => pending.forEach((timer) => window.clearTimeout(timer))
  }, [])
  const later = useCallback((ms: number, run: () => void) => {
    const timer = window.setTimeout(() => {
      timers.current.delete(timer)
      run()
    }, ms)
    timers.current.add(timer)
  }, [])

  const latest = useRef({ glance, settling })
  latest.current = { glance, settling }

  const open = useCallback(
    (sessionId: string) => {
      dispatch(openSession({ sessionId }))
      onLeave()
    },
    [dispatch, onLeave],
  )

  const answer = useCallback(
    (summary: SessionSummary, approval: Approval, choice: Answer) => {
      const { glance, settling } = latest.current
      if (settling.some((entry) => entry.sessionId === summary.id)) return
      const next = afterAnswer(
        glance.needsYou,
        summary.id,
        new Set(settling.map((entry) => entry.sessionId)),
      )
      const focus = document.activeElement
      const hadKeyboard =
        focus instanceof HTMLElement &&
        focus.closest<HTMLElement>("[data-overview-item]")?.dataset.overviewItem ===
          summary.id
      const phase = (to: Settling["phase"]) =>
        setSettling((all) =>
          all.map((entry) =>
            entry.sessionId === summary.id ? { ...entry, phase: to } : entry,
          ),
        )
      const release = () =>
        setSettling((all) => all.filter((entry) => entry.sessionId !== summary.id))
      setSettling((all) => [
        ...all,
        {
          sessionId: summary.id,
          updatedAt: summary.updatedAt,
          summary,
          approval,
          answer: choice,
          phase: "answering",
        },
      ])
      setFailures((all) => without(all, summary.id))
      // The keyboard moves on at once, so a run of requests is answered in a row.
      if (hadKeyboard) focusItem(next)
      void dispatch(
        answerRequest({ sessionId: summary.id, approvalId: approval.id, answer: choice }),
      ).then((sent) => {
        if (sent.kind === "failed") {
          release()
          setFailures((all) =>
            new Map(all).set(summary.id, {
              approvalId: approval.id,
              reason: sent.reason,
            }),
          )
          return
        }
        phase("settled")
        setSaid(`${answeredLabels[choice]}: ${summary.title}`)
        later(settledFor, () => {
          phase("leaving")
          later(durationOf(list.current, "--desktop-base"), release)
        })
      })
    },
    [dispatch, focusItem, later],
  )

  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    // A menu opened from the list (the filter) is portalled elsewhere in the
    // page but bubbles here through React: its keys are its own.
    if (!(event.target instanceof Node) || !event.currentTarget.contains(event.target))
      return
    const binding = overviewKeys.find((candidate) =>
      matchesChord(event.nativeEvent, candidate.chord),
    )
    if (!binding) return
    if (binding.command === "leave") {
      event.preventDefault()
      onLeave()
      return
    }
    if (binding.command === "reply") {
      event.preventDefault()
      const from =
        event.target instanceof HTMLElement
          ? event.target.closest<HTMLElement>("[data-overview-item]")?.dataset
              .overviewItem
          : undefined
      const to = from ?? current
      if (to !== null) reply(to)
      return
    }
    const step = steps[binding.command]
    if (!step) return
    event.preventDefault()
    // From the item the keyboard is in; from before the first when it rests on the list.
    const from =
      event.target instanceof HTMLElement
        ? (event.target.closest<HTMLElement>("[data-overview-item]")?.dataset
            .overviewItem ?? null)
        : null
    const to = stepFrom(order, from, step)
    if (to !== null) focusItem(to)
  }

  // ⌥ held turns every Allow into Always Allow, as it shows the other choice in a Mac menu.
  const [alt, setAlt] = useState(false)
  useEffect(() => {
    const follow = (event: KeyboardEvent) => setAlt(event.altKey)
    const release = () => setAlt(false)
    window.addEventListener("keydown", follow)
    window.addEventListener("keyup", follow)
    window.addEventListener("blur", release)
    return () => {
      window.removeEventListener("keydown", follow)
      window.removeEventListener("keyup", follow)
      window.removeEventListener("blur", release)
    }
  }, [])

  const settlingOf = useMemo(
    () => new Map(settling.map((entry) => [entry.sessionId, entry])),
    [settling],
  )

  // Wide enough, the peek sits beside the list and follows its choice; where
  // it is not, a chosen row opens its peek beneath it.
  const surface = useRef<HTMLDivElement>(null)
  const split = useAtLeastWide(surface, splitWidth)
  const [expanded, setExpanded] = useState<string | null>(null)
  const splitNow = useRef(split)
  splitNow.current = split
  const choose = useCallback((sessionId: string) => {
    setActive(sessionId)
    if (!splitNow.current) setExpanded((open) => (open === sessionId ? null : sessionId))
  }, [])

  // ⌘R: the keyboard goes to the reply pill of the session it is on, its
  // peek opened first where the peek is beneath the row.
  const section = useRef<HTMLElement>(null)
  const reply = (sessionId: string) => {
    setActive(sessionId)
    if (!splitNow.current) setExpanded(sessionId)
    requestAnimationFrame(() =>
      section.current
        ?.querySelector<HTMLTextAreaElement>(
          `[data-reply-for="${CSS.escape(sessionId)}"] textarea`,
        )
        ?.focus(),
    )
  }
  const currentNow = useRef(current)
  currentNow.current = current
  // Escape in a pill: back to the row it replies to.
  const leaveReply = useCallback(() => focusItem(currentNow.current), [focusItem])

  return (
    <section
      ref={section}
      className="agents-overview"
      aria-label="Agents"
      data-alt={alt || undefined}
      data-split={split || undefined}
    >
      <div className="agents-overview-surface" ref={surface}>
        <div className="agents-overview-scroll">
          <div
            ref={list}
            className="agents-overview-column"
            tabIndex={-1}
            onKeyDown={onKeyDown}
          >
            <header className="agents-overview-header">
              <div className="agents-overview-title">
                <h1>Agents</h1>
                <FilterMenu filter={filter} onChange={setFilter} />
              </div>
              <p>{ready ? glanceLine(glance) : "\u00a0"}</p>
            </header>
            {ready ? (
              <Groups
                glance={glance}
                current={current}
                selected={split ? current : null}
                expanded={split ? null : expanded}
                settlingOf={settlingOf}
                failures={failures}
                onFocusItem={setActive}
                onChoose={choose}
                onOpen={open}
                onAnswer={answer}
                onLeaveReply={leaveReply}
                onShowAll={
                  filter.scope === "all"
                    ? undefined
                    : () => setFilter({ ...filter, scope: "all" })
                }
              />
            ) : null}
            <p className="agents-overview-said" aria-live="polite">
              {said}
            </p>
          </div>
        </div>
        {split && current !== null ? (
          <aside className="agents-overview-peek" aria-label="Peek">
            <SessionPeek
              key={current}
              sessionId={current}
              settling={settlingOf.get(current)}
              failure={failures.get(current)}
              onOpen={open}
              onAnswer={answer}
              onLeaveReply={leaveReply}
            />
          </aside>
        ) : null}
      </div>
    </section>
  )
}

/** How wide the overview must be to hold its peek beside the list. */
const splitWidth = 820

interface AnswerFailure {
  readonly approvalId: string
  readonly reason: WorkspaceFailureReason
}

function without<T>(map: ReadonlyMap<string, T>, key: string): ReadonlyMap<string, T> {
  if (!map.has(key)) return map
  const next = new Map(map)
  next.delete(key)
  return next
}

function Groups({
  glance,
  current,
  selected,
  expanded,
  settlingOf,
  failures,
  onFocusItem,
  onChoose,
  onOpen,
  onAnswer,
  onLeaveReply,
  onShowAll,
}: {
  glance: AgentsGlance
  current: string | null
  selected: string | null
  expanded: string | null
  settlingOf: ReadonlyMap<string, Settling>
  failures: ReadonlyMap<string, AnswerFailure>
  onFocusItem: (id: string) => void
  onChoose: (id: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: (summary: SessionSummary, approval: Approval, choice: Answer) => void
  onLeaveReply: () => void
  /** Widens the filter to every session; absent when it already lists them all. */
  onShowAll: (() => void) | undefined
}) {
  const row = (sessionId: string) => ({
    sessionId,
    current: sessionId === current,
    selected: sessionId === selected,
    expanded: sessionId === expanded,
    onFocus: onFocusItem,
    onChoose,
    onOpen,
    onAnswer,
    onLeaveReply,
  })
  return (
    <>
      <section className="agents-overview-group" aria-labelledby="agents-needs-you">
        <h2 id="agents-needs-you" data-reflow="title:needs-you">
          Needs you
        </h2>
        {glance.needsYou.length > 0 ? (
          <ul role="list" className="agents-overview-list">
            {glance.needsYou.map((sessionId) => (
              <RequestRow
                key={sessionId}
                {...row(sessionId)}
                settling={settlingOf.get(sessionId)}
                failure={failures.get(sessionId)}
              />
            ))}
          </ul>
        ) : (
          <AllClear />
        )}
      </section>
      {glance.working.length > 0 ? (
        <section className="agents-overview-group" aria-labelledby="agents-working">
          <h2 id="agents-working" data-reflow="title:working">
            Working
          </h2>
          <ul role="list" className="agents-overview-list">
            {glance.working.map((sessionId) => (
              <SessionRow key={sessionId} {...row(sessionId)} kind="working" />
            ))}
          </ul>
        </section>
      ) : null}
      {glance.finished.length > 0 ? (
        <section className="agents-overview-group" aria-labelledby="agents-finished">
          <h2 id="agents-finished" data-reflow="title:finished">
            Finished
          </h2>
          <ul role="list" className="agents-overview-list">
            {glance.finished.map((sessionId) => (
              <SessionRow key={sessionId} {...row(sessionId)} kind="finished" />
            ))}
          </ul>
        </section>
      ) : null}
      {glance.earlier.length > 0 ? (
        <section className="agents-overview-group" aria-labelledby="agents-earlier">
          <h2 id="agents-earlier" data-reflow="title:earlier">
            Earlier
          </h2>
          <ul role="list" className="agents-overview-list">
            {glance.earlier.map((sessionId) => (
              <SessionRow key={sessionId} {...row(sessionId)} kind="earlier" />
            ))}
          </ul>
        </section>
      ) : null}
      {glance.hidden > 0 ? (
        <p className="agents-overview-resting" data-reflow="hidden">
          {glance.hidden} more {glance.hidden === 1 ? "session" : "sessions"} outside this
          view
          {onShowAll ? (
            <>
              {" · "}
              <button type="button" onClick={onShowAll}>
                Show All
              </button>
            </>
          ) : null}
        </p>
      ) : null}
    </>
  )
}
