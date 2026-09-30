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
import { isMac } from "../../../adapters/platform"
import { matchesChord } from "../../../model/keyboard"
import {
  approve,
  deny,
  filterOverview,
  openSession,
  selectInOverview,
  showOverviewGroup,
  type AnswerOutcome,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectGlance,
  selectOverviewFilter,
  selectOverviewGroup,
  selectOverviewSelected,
  selectReady,
} from "../../adapters/store/selectors"
import {
  glanceCounts,
  quietLine,
  quietOf,
  readingOrder,
  sameGlance,
  type AgentsGlance,
  type AgentsGroup,
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
import { ReplyCaret } from "./reply-pill"
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
  const group = useWorkspaceSelector(selectOverviewGroup)
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
  // A count shows its group alone; chosen again, every group.
  const toggleGroup = useCallback(
    (chosen: AgentsGroup) =>
      dispatch(showOverviewGroup({ group: chosen === group ? null : chosen })),
    [dispatch, group],
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
    (summary, approval, choice, at) => {
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
        // Timed from the answering press itself, on the clock a later press is stamped on.
        movedAt.current = at
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
      matchesChord(event.nativeEvent, candidate.chord, isMac),
    )
    if (!binding) return
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
  const following = useDeferredValue(current)
  // ⌘R draws the peek of the session it replies to at once, not a step
  // behind, so the pill is there for the next key typed.
  const [replyingTo, setReplyingTo] = useState<string | null>(null)
  const peeked = replyingTo !== null && replyingTo === current ? current : following
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

  // The session whose reply pill holds the caret, or is to take it: a pill
  // drawn for it takes the caret as it mounts (`ReplyCaret`).
  const caret = useRef<string | null>(null)
  // ⌘R: the keyboard goes to the reply pill of the session it is on — the
  // one on the page, or the one drawn for it now: its peek, beside the list
  // or opened beneath the row, drawn in this same update.
  const reply = (sessionId: string) => {
    caret.current = sessionId
    choose(sessionId)
    setReplyingTo(sessionId)
    setPeekDrawn(true)
    if (!splitNow.current) setExpanded(sessionId)
    section.current
      ?.querySelector<HTMLTextAreaElement>(
        `[data-reply-for="${CSS.escape(sessionId)}"] textarea`,
      )
      ?.focus()
  }
  // Focus landing anywhere but that session's pill lets the caret go: the
  // person moved it (Escape, a click, Tab).
  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const pill =
        event.target instanceof Element
          ? event.target.closest<HTMLElement>("[data-reply-for]")
          : null
      if (pill?.dataset.replyFor === caret.current) return
      caret.current = null
      setReplyingTo(null)
    }
    document.addEventListener("focusin", onFocusIn)
    return () => document.removeEventListener("focusin", onFocusIn)
  }, [])

  // Escape leaves whenever the overview is open — wherever the keyboard is,
  // even before it has landed on a row — but for Escape in a menu or a
  // dialog over it, which is theirs, and under Settings, whose keys are its
  // own; and, with one group shown alone, the first Escape shows every group
  // again and the next leaves.
  const leaveNow = useRef(onLeave)
  leaveNow.current = onLeave
  const groupNow = useRef(group)
  groupNow.current = group
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      const leave = overviewKeys.find(
        (binding) =>
          binding.command === "leave" && matchesChord(event, binding.chord, isMac),
      )
      if (!leave) return
      if (section.current?.closest("[inert]")) return
      if (
        event.target instanceof Element &&
        event.target.closest('[role="dialog"], [role="menu"], [role="listbox"]')
      )
        return
      event.preventDefault()
      if (groupNow.current !== null) dispatch(showOverviewGroup({ group: null }))
      else leaveNow.current()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [dispatch])
  // Escape in a pill, or a count the keyboard was on gone from the line: to
  // the list, on the current row, or at its top where it lists none.
  const leaveReply = useCallback(() => focusItem(currentNow.current), [focusItem])

  // Focus the overview loses, it gives back — one rule, whatever took the
  // element away: a session changing group (its row drawn anew under another
  // heading), the peek beneath a row or beside the list going, a count
  // leaving the line, Show All once nothing is left out. The element leaves
  // the page with focus on it, and focus falls to the page's body, where no
  // key is heard. It goes instead to that element's session's row (a row, or
  // the peek beneath it) — or, the session gone from the list or the element
  // no row's (a count, the peek beside the list, which shows the current
  // session), to the current row, or the list where it lists none — before
  // the frame is painted: a mutation is answered at the microtask after the
  // change, whoever made it. Focus that has already landed elsewhere (a reply
  // pill drawn anew takes its caret itself) is held there, and nothing is due.
  //
  // What the person does is left alone. Focus they move (a click on text)
  // lets the element go; focus the window takes with it (another app) leaves
  // the element focused, and it is kept.
  const lastFocus = useRef<{ element: HTMLElement; sessionId: string | null } | null>(
    null,
  )
  const orderNow = useRef(order)
  orderNow.current = order
  const giveBack = useRef(() => {})
  giveBack.current = () => {
    const last = lastFocus.current
    if (last === null || last.element.isConnected) return
    lastFocus.current = null
    focusItem(
      last.sessionId !== null && orderNow.current.includes(last.sessionId)
        ? last.sessionId
        : currentNow.current,
    )
  }
  useEffect(() => {
    const root = section.current
    if (!root) return
    const sessionOf = (element: HTMLElement) =>
      element
        .closest<HTMLElement>(".agents-row-item")
        ?.querySelector<HTMLElement>("[data-overview-item]")?.dataset.overviewItem ?? null
    const onFocusIn = (event: FocusEvent) => {
      const element = event.target
      lastFocus.current =
        element instanceof HTMLElement && root.contains(element)
          ? { element, sessionId: sessionOf(element) }
          : null
    }
    const onFocusOut = (event: FocusEvent) => {
      const element = event.target
      if (!(element instanceof HTMLElement)) return
      // Decided once the change that moved focus has settled: Chromium tells
      // an element it is losing focus as it is taken off the page, before it
      // has gone, so only afterwards can a removal be told from a person
      // leaving. Still on the page and no longer focused, it was left; the
      // window going to another app leaves it the focused element, and it is
      // kept. A click away in the same tick as the element's removal looks
      // like the removal, and focus is given back.
      queueMicrotask(() => {
        if (lastFocus.current?.element !== element || !element.isConnected) return
        if (document.activeElement !== element) lastFocus.current = null
      })
    }
    const removals = new MutationObserver(() => giveBack.current())
    removals.observe(root, { childList: true, subtree: true })
    document.addEventListener("focusin", onFocusIn)
    document.addEventListener("focusout", onFocusOut)
    return () => {
      removals.disconnect()
      document.removeEventListener("focusin", onFocusIn)
      document.removeEventListener("focusout", onFocusOut)
    }
  }, [])

  return (
    <ReplyCaret.Provider value={caret}>
      <section
        ref={section}
        className="agents-overview"
        aria-label="Agents"
        data-alt={alt || undefined}
        data-split={split || undefined}
      >
        <div className="agents-overview-surface">
          {/* The header stays where it is; only the list under it scrolls. */}
          <div className="agents-overview-side">
            <header className="agents-overview-header">
              <div className="agents-overview-title">
                <h1>Agents</h1>
                <FilterMenu filter={filter} onChange={setFilter} />
              </div>
              <Counts ready={ready} glance={glance} onToggle={toggleGroup} />
            </header>
            <div className="agents-overview-scroll">
              <div
                ref={list}
                className="agents-overview-column"
                tabIndex={-1}
                onKeyDown={onKeyDown}
              >
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
                    onShowAll={() => setFilter(everySession)}
                  />
                ) : null}
                <p className="agents-overview-said" aria-live="polite">
                  {said}
                </p>
              </div>
            </div>
          </div>
          {split && peeked !== null ? (
            <aside className="agents-overview-peek" aria-label="Peek">
              {peekDrawn ? (
                <SessionPeek
                  key={peeked}
                  sessionId={peeked}
                  placement="beside"
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
    </ReplyCaret.Provider>
  )
}

/** How wide the overview's layer must be to hold its peek beside the list. */
export const splitWidth = 820

/** What Show All chooses: every session, at any time, under any tag. */
const everySession: AgentsFilter = { scope: "all", range: "any", tags: [] }

/**
 * The header's counts, each a toggle that shows its group alone — pressed
 * while it does — and, chosen again, every group. Nothing to count, one
 * quiet line; not ready, a line's room held so the header does not move.
 *
 * A count goes from the line when its group has none the filter lets
 * through and is not shown alone — let go at nought, or emptied by the
 * source. When the keyboard was on it, the overview gives it back to the
 * list in the same frame, as it does any focus it loses.
 */
function Counts({
  ready,
  glance,
  onToggle,
}: {
  ready: boolean
  glance: AgentsGlance
  onToggle: (group: AgentsGroup) => void
}) {
  const counts = glanceCounts(glance)
  if (!ready) return <p className="agents-overview-counts">{"\u00a0"}</p>
  if (counts.length === 0) return <p className="agents-overview-counts">{quietLine}</p>
  return (
    <p className="agents-overview-counts" role="group" aria-label="Show only">
      {counts.map(({ group, label }, index) => (
        <span key={group} className="agents-overview-count">
          {index > 0 ? <span aria-hidden="true">{" · "}</span> : null}
          <button
            type="button"
            aria-pressed={glance.group === group}
            data-group={group}
            onClick={() => onToggle(group)}
          >
            {label}
          </button>
        </span>
      ))}
    </p>
  )
}

/** What a group shown alone says when it lists nothing. */
const emptyGroup: Readonly<Record<AgentsGroup, string>> = {
  needsYou: "Nothing needs you",
  working: "Nothing is working",
  finished: "Nothing has finished",
  earlier: "Nothing from earlier",
}

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
  /** Widens the filter to every session, which lists all it kept out. */
  onShowAll: () => void
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
  // Nothing listed at all, or nothing of the group shown alone: one quiet
  // line, never an empty page (`quietOf`).
  const quiet = quietOf(glance)
  return (
    <>
      {quiet === "all" ? (
        <AllClear />
      ) : quiet !== null ? (
        <p className="agents-overview-resting" data-reflow="empty">
          {emptyGroup[quiet]}
        </p>
      ) : null}
      {/* Shown only while something waits: an empty section is nothing to review. */}
      {glance.needsYou.length > 0 ? (
        <section className="agents-overview-group" aria-labelledby="agents-needs-you">
          <h2 id="agents-needs-you" data-reflow="title:needs-you">
            Needs you
          </h2>
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
        </section>
      ) : null}
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
          {" · "}
          <button type="button" onClick={onShowAll}>
            Show All
          </button>
        </p>
      ) : null}
    </>
  )
}
