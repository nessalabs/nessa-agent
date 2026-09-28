import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import { reducedMotion } from "../../../adapters/motion-preference"
import { useNow } from "../../adapters/dom/clock"
import { durationToken } from "../../../adapters/motion"
import { useReflow } from "../../adapters/dom/overview-reflow"
import { matchesChord } from "../../adapters/dom/shortcuts"
import {
  approve,
  deny,
  filterOverview,
  openSession,
  selectInOverview,
  type AnswerOutcome,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectGlance,
  selectOverviewFilter,
  selectOverviewSelected,
  selectReady,
} from "../../adapters/store/selectors"
import {
  glanceLine,
  readingOrder,
  sameGlance,
  type AgentsGlance,
  type Held,
} from "../../model/overview/agents-glance"
import type { AgentsFilter } from "../../model/overview/filter"
import {
  afterAnswer,
  stepFrom,
  takesAnswerKey,
  type Step,
} from "../../model/overview/walk"
import { AllClear } from "./all-clear"
import { FilterMenu } from "./filter-menu"
import { overviewKeys } from "./overview-keys"
import { RequestRow } from "./request-row"
import { SessionPeek } from "./session-peek"
import { SessionRow } from "./session-row"
import { answeredLabels, type OnAnswer, type Settling } from "./settling"
import "./overview.css"

/** How long an answered request says what became of it before it goes. */
const settledFor = 700

const steps: Partial<Record<string, Step>> = {
  next: "next",
  previous: "previous",
  first: "first",
  last: "last",
}

/**
 * Every agent at a glance, in the content region: what waits on the person,
 * with each approval answerable where it stands; what is working; what
 * finished unseen — and a peek at the chosen one, beside the list where
 * there is room (`split`) and beneath its row where there is not, to check
 * in on it without opening it. The keyboard walks the list with the arrows
 * (the peek follows), opens with ↩ and answers with ⌘↩, ⌥⌘↩ and ⌘⌫, moving on
 * to the next request as each one settles and leaves.
 *
 * What it lists and which session is chosen are the workspace's
 * (`filterOverview`, `selectInOverview`), and an answer is the workspace's
 * own `approve` or `deny`, as from a pane's card: the overview holds only
 * how a row settles in place.
 */
export function AgentsOverview({
  split,
  onLeave,
}: {
  /** Wide enough for the peek beside the list, as its layer measured before it opened. */
  split: boolean
  onLeave: () => void
}) {
  const dispatch = useWorkspaceDispatch()
  const ready = useWorkspaceSelector(selectReady)
  const filter = useWorkspaceSelector(selectOverviewFilter)
  const selected = useWorkspaceSelector(selectOverviewSelected)
  const [settling, setSettling] = useState<readonly Settling[]>([])
  const [said, setSaid] = useState("")
  const held = useMemo<readonly Held[]>(
    () =>
      settling.map(({ sessionId, summary }) => ({
        sessionId,
        updatedAt: summary.updatedAt,
      })),
    [settling],
  )
  const now = useNow(60_000)
  const glance = useWorkspaceSelector(
    (state) => selectGlance(state, held, now),
    sameGlance,
  )
  const order = useMemo(() => readingOrder(glance), [glance])
  // The workspace keeps a listed session chosen while it lists one (`keepOverviewChoice`).
  const current = selected !== null && order.includes(selected) ? selected : null
  const choose = useCallback(
    (sessionId: string) => dispatch(selectInOverview({ sessionId })),
    [dispatch],
  )
  const setFilter = useCallback(
    (next: AgentsFilter) => dispatch(filterOverview({ filter: next })),
    [dispatch],
  )
  const [expanded, setExpanded] = useState<string | null>(null)
  const list = useRef<HTMLDivElement>(null)
  // Only a change of what is listed, or where a peek opens, re-flows the list.
  useReflow(list, `${order.join(",")}|${split ? "" : (expanded ?? "")}`)

  const section = useRef<HTMLElement>(null)
  const focusItem = useCallback(
    (id: string | null) => {
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
      const item = column.querySelector<HTMLElement>(
        `[data-overview-item="${CSS.escape(id)}"]`,
      )
      // Focused, the row chooses itself (`onFocus`), so the peek follows the
      // keyboard. It is brought into view a frame on, once the frame has laid
      // the page out on its own: scrolling now would make it lay out early.
      if (!item) return choose(id)
      item.focus({ preventScroll: true })
      requestAnimationFrame(() =>
        item.scrollIntoView({
          block: "nearest",
          behavior: reducedMotion() ? "auto" : "smooth",
        }),
      )
    },
    [choose],
  )

  const currentNow = useRef(current)
  currentNow.current = current

  // Opened — from the sidebar, ⌘0 or an agent — the keyboard is here, on the
  // current row, once the overview has been laid out and drawn, so the
  // caret's arrival never makes its first frame lay the page out early. It
  // lands once: a render meanwhile does not move it again, and a cancelled
  // try (StrictMode's second mount, a quick leave) leaves the next to land.
  const landed = useRef(false)
  useEffect(() => {
    if (landed.current || !ready) return
    // A frame asked for now is this frame's; the one after it is the next.
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => {
        landed.current = true
        focusItem(currentNow.current)
      })
    })
    return () => cancelAnimationFrame(frame)
  }, [ready, focusItem])

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

  // When the keyboard last moved on by answering: a press before the person
  // could see where it went is not taken (`takesAnswerKey`). A press is timed
  // from when it was made (`timeStamp`, on `performance.now()`'s clock), not
  // from when the page got to it, so one queued behind other work still counts.
  const movedAt = useRef<number | null>(null)
  const takesKey = useCallback(
    (event: KeyboardEvent) =>
      takesAnswerKey({ repeat: event.repeat, at: event.timeStamp }, movedAt.current),
    [],
  )

  const latest = useRef({ glance, settling })
  latest.current = { glance, settling }

  const open = useCallback(
    (sessionId: string) => {
      // Going to a session is going to the panes (`navigated`).
      dispatch(openSession({ sessionId }))
      onLeave()
    },
    [dispatch, onLeave],
  )

  const answer = useCallback<OnAnswer>(
    (summary, approval, choice) => {
      const { glance, settling } = latest.current
      if (settling.some((entry) => entry.sessionId === summary.id)) return
      const next = afterAnswer(
        glance.needsYou,
        summary.id,
        new Set(settling.map((entry) => entry.sessionId)),
      )
      // Answered from the overview — by a key on its row, or a click in its
      // peek — the keyboard moves on at once, so a run of requests is
      // answered in a row and focus is never left on a button that goes.
      const focus = document.activeElement
      const here =
        focus === null ||
        focus === document.body ||
        (focus instanceof Node && section.current?.contains(focus) === true)
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
        { sessionId: summary.id, summary, approval, choice, phase: "answering" },
      ])
      if (here) {
        focusItem(next)
        movedAt.current = performance.now()
      }
      const asked =
        choice === "deny"
          ? deny({ sessionId: summary.id, approvalId: approval.id, initiator: "person" })
          : approve({
              sessionId: summary.id,
              approvalId: approval.id,
              scope: choice,
              initiator: "person",
            })
      void dispatch(asked).then((outcome: AnswerOutcome) => {
        // Refused, not confirmed, already on its way from a pane, or no longer
        // asked: the row comes back as the workspace holds it — asking again,
        // saying why (`selectAnswer`), or moved on.
        if (outcome !== "sent") {
          release()
          return
        }
        phase("settled")
        setSaid(`${answeredLabels[choice]}: ${summary.title}`)
        later(settledFor, () => {
          phase("leaving")
          later(list.current ? durationToken(list.current, "--desktop-base") : 0, release)
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
      if (to !== null && to !== undefined) reply(to)
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

  // The overview arrives fading in from nothing; the peek beside its list is
  // drawn a frame later, so the frame it opens on lays out the list alone.
  const [peekDrawn, setPeekDrawn] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setPeekDrawn(true))
    return () => cancelAnimationFrame(frame)
  }, [])

  // Wide enough, the peek sits beside the list and follows its choice — a
  // step behind the list, so a key that moves on (an arrow, an answer) draws
  // the list's change in its own frame and the next session's peek after it;
  // where it is not, a chosen row opens its peek beneath it.
  const peeked = useDeferredValue(current)
  const splitNow = useRef(split)
  splitNow.current = split
  const pick = useCallback(
    (sessionId: string) => {
      choose(sessionId)
      if (!splitNow.current)
        setExpanded((open) => (open === sessionId ? null : sessionId))
    },
    [choose],
  )

  // ⌘R: the keyboard goes to the reply pill of the session it is on, its
  // peek opened first where the peek is beneath the row.
  const reply = (sessionId: string) => {
    choose(sessionId)
    if (!splitNow.current) setExpanded(sessionId)
    requestAnimationFrame(() =>
      section.current
        ?.querySelector<HTMLTextAreaElement>(
          `[data-reply-for="${CSS.escape(sessionId)}"] textarea`,
        )
        ?.focus(),
    )
  }
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
      <div className="agents-overview-surface">
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
                onFocusItem={choose}
                onChoose={pick}
                onOpen={open}
                onAnswer={answer}
                onLeaveReply={leaveReply}
                takesKey={takesKey}
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
        {split && peeked !== null ? (
          <aside className="agents-overview-peek" aria-label="Peek">
            {peekDrawn ? (
              <SessionPeek
                key={peeked}
                sessionId={peeked}
                settling={settlingOf.get(peeked)}
                onOpen={open}
                onAnswer={answer}
                onLeaveReply={leaveReply}
              />
            ) : null}
          </aside>
        ) : null}
      </div>
    </section>
  )
}

/** How wide the overview's layer must be to hold its peek beside the list. */
export const splitWidth = 820

function Groups({
  glance,
  current,
  selected,
  expanded,
  settlingOf,
  onFocusItem,
  onChoose,
  onOpen,
  onAnswer,
  onLeaveReply,
  takesKey,
  onShowAll,
}: {
  glance: AgentsGlance
  current: string | null
  selected: string | null
  expanded: string | null
  settlingOf: ReadonlyMap<string, Settling>
  onFocusItem: (id: string) => void
  onChoose: (id: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: OnAnswer
  onLeaveReply: () => void
  takesKey: (event: KeyboardEvent) => boolean
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
                takesKey={takesKey}
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
