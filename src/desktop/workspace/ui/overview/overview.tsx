import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { flushSync } from "react-dom"
import { reducedMotion } from "../../../adapters/motion-preference"
import { useNow } from "../../adapters/dom/clock"
import { leaveInPlace, useReflow } from "../../adapters/dom/overview-reflow"
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
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectGlance,
  selectOverviewFilter,
  selectOverviewGroup,
  selectOverviewSelected,
  selectReady,
  selectSession,
  selectTranscript,
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
import { ResizeEdge } from "../../../ui/resize-edge"
import { answeredLabels, type OnAnswer, type Settling } from "./settling"
import "./overview.css"

/**
 * The longest a closed row waits for the workspace to stop asking what it
 * answered, before it is let go to show what the workspace holds.
 */
const heldAtMost = 2000

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
  listWidth,
  onListWidth,
  onLeave,
}: {
  /** Wide enough for the peek beside the list, as its layer measured before it opened. */
  split: boolean
  /** The list's width beside the peek, as dragged; null for the even split. */
  listWidth: number | null
  onListWidth: (width: number | null) => void
  onLeave: () => void
}) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
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
  // The list beside the peek, resized by its edge: the widths as drawn, for
  // the edge to say where it stands, taken as they change (no layout is read
  // for it) — and as they were when a drag or key press began.
  const surface = useRef<HTMLDivElement>(null)
  const side = useRef<HTMLDivElement>(null)
  const [drawn, setDrawn] = useState({ list: 0, surface: 0 })
  const dragFrom = useRef({ list: 0, surface: 0 })
  useEffect(() => {
    const room = surface.current
    const list = side.current
    if (!room || !list || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(() =>
      setDrawn((was) => {
        const now = { list: list.offsetWidth, surface: room.offsetWidth }
        return was.list === now.list && was.surface === now.surface ? was : now
      }),
    )
    observer.observe(room)
    observer.observe(list)
    return () => observer.disconnect()
  }, [])
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
  const watches = useRef(new Set<() => void>())
  // An answer that settles after the overview closed must not start a watch or timer.
  const mounted = useRef(true)
  useEffect(() => {
    const pending = timers.current
    const watching = watches.current
    mounted.current = true
    return () => {
      mounted.current = false
      pending.forEach((timer) => window.clearTimeout(timer))
      watching.forEach((stop) => stop())
    }
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
    (summary, approval, option, at) => {
      const choice = option.choice
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
          ? deny({
              sessionId: summary.id,
              approvalId: approval.id,
              initiator: "person",
              optionId: option.id,
            })
          : approve({
              sessionId: summary.id,
              approvalId: approval.id,
              scope: choice,
              initiator: "person",
              optionId: option.id,
            })
      void dispatch(asked).then((outcome: AnswerOutcome) => {
        if (!mounted.current) return
        // Refused, not confirmed, already on its way from a pane, or no longer
        // asked: the row comes back as the workspace holds it — asking again,
        // saying why (`selectAnswer`), or moved on.
        if (outcome !== "sent") {
          release()
          return
        }
        // Taken: what became of it is said to a screen reader, and the row
        // waits, quiet, until the source moves its session on — "sent" can
        // come before the session's own move — so it never springs back into
        // Needs you. Then it leaves its place as it arrives in the new one,
        // in the same beat (`leaveInPlace`). An update that never comes lets
        // it go after `heldAtMost`, shown as the workspace holds it.
        phase("leaving")
        setSaid(`${answeredLabels[choice]}: ${summary.title}`)
        // Still where it was: the session waits on the person (what lists it
        // in Needs you) and asks nothing new — the approval it let go can
        // arrive before the session's own move on.
        const asks = () => {
          const state = store.getState()
          const asked = selectTranscript(state, summary.id)?.approval
          return (
            selectSession(state, summary.id)?.status === "needs-you" &&
            (asked == null || asked.id === approval.id)
          )
        }
        const moveOn = () => {
          // Its copy is taken as it stands leaving — its answers put away —
          // not as it stood a moment ago, still answering.
          flushSync(() => phase("leaving"))
          const column = list.current
          const item = column
            ?.querySelector(`[data-overview-item="${CSS.escape(summary.id)}"]`)
            ?.closest<HTMLElement>(".agents-row-item")
          if (column && item) leaveInPlace(column, item)
          // Taken away in this same task, so the re-flow — the copy going,
          // the gap closing, the session arriving — starts in the next frame.
          flushSync(release)
        }
        if (!asks()) return moveOn()
        const done = (moved: boolean) => {
          if (!watches.current.delete(stop)) return
          stop()
          if (moved) moveOn()
          else release()
        }
        const stop = store.subscribe(() => {
          if (!asks()) done(true)
        })
        watches.current.add(stop)
        later(heldAtMost, () => done(false))
      })
    },
    [dispatch, focusItem, later, store],
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

  // ⌥ held turns Allow into Always Allow where the review offers it, as ⌥ shows the other choice in a Mac menu.
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
  // Escape in a pill: to the list, on the current row, or at its top where
  // it lists none.
  const leaveReply = useCallback(() => focusItem(currentNow.current), [focusItem])

  // Focus the overview loses, it gives back — one rule, whatever lost it: a
  // session changing group (its row drawn anew under another heading) or
  // moving within one (a row reordered as its session streams), the peek
  // beneath a row or beside the list going, a count leaving the line, Show
  // All once nothing is left out. Each takes the focused element off the
  // page, if only to put it back, and focus falls to the page's body, where
  // no key is heard. It goes instead to that element's session's row (a row,
  // or the peek beneath it) — or, the session gone from the list or the
  // element no row's (a count, the peek beside the list, which shows the
  // current session), to the current row, or the list where it lists none —
  // on the frame after the change: a mutation is answered at the microtask
  // after it, whoever made it, and focus lands once the page is laid out.
  //
  // Focus the person moves is theirs. A press anywhere but on the focused
  // element that takes focus from it (a click on text) lets it go as focus
  // leaves, so what follows in the same task does not take it back. Focus
  // lost while the window is away (another app, or tabbed out of the page)
  // is given back when the window has it again. Focus that has landed
  // elsewhere (a reply pill drawn anew takes its caret itself) is left there.
  const lastFocus = useRef<{ element: HTMLElement; sessionId: string | null } | null>(
    null,
  )
  const orderNow = useRef(order)
  orderNow.current = order
  // Every press on the page, counted: one after focus is due back (the click
  // that brings the window back, say) is the person choosing, and wins.
  const presses = useRef(0)
  const giveBack = useRef(() => {})
  giveBack.current = () => {
    const last = lastFocus.current
    const focus = document.activeElement
    if (last === null || (focus !== null && focus !== document.body)) return
    // Away from the window (another app, or tabbed out of the page): due
    // when it comes back, not now.
    if (!document.hasFocus()) return
    lastFocus.current = null
    const { element, sessionId } = last
    const pressed = presses.current
    // On the next frame, once the page is laid out: WebKit scrolls an element
    // focused while its layout is pending into view, `preventScroll` or not.
    // Focus only — the list stays where the person has scrolled it, even as
    // rows reorder under them; walking the list is what brings a row into
    // view. Given only if nothing has taken focus meanwhile, and to the row
    // the list holds then, after every change before the frame.
    requestAnimationFrame(() => {
      const focus = document.activeElement
      if (presses.current !== pressed || (focus !== null && focus !== document.body))
        return
      const listed = sessionId !== null && orderNow.current.includes(sessionId)
      // Moved, not taken away (a row, or a reply pill beneath it, reordered
      // as sessions stream): focus goes back to the element itself, caret
      // and all, so the key typed next lands where the one before did — if
      // it can still take it; a button disabled as it acted cannot.
      if (listed && element.isConnected) {
        element.focus({ preventScroll: true })
        if (document.activeElement === element) return
      }
      const id = listed ? sessionId : currentNow.current
      const column = list.current
      const item =
        id === null
          ? null
          : column?.querySelector<HTMLElement>(`[data-overview-item="${CSS.escape(id)}"]`)
      ;(item ?? column)?.focus({ preventScroll: true })
    })
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
    // A press elsewhere is the person moving focus only if focus moves as
    // part of that press. Focus moves as the default action of `mousedown` —
    // for a mouse, and for the mouse events a touch or a pen sends as it
    // lifts — so the press is taken at `mousedown`, and counts for that task
    // only. A press that leaves focus where it is (the titlebar's drag strip,
    // the edge strip, a scrollbar, a `mousedown` cancelled) changes nothing,
    // and a row taken away while a press is held — Chromium tells it it is
    // losing focus then too — arrives in a later task and is given back.
    // Nothing waits on the release, which a native window drag may swallow.
    let pressing = false
    const onPress = (event: MouseEvent) => {
      presses.current += 1
      const last = lastFocus.current
      pressing =
        last !== null &&
        !(event.target instanceof Node && last.element.contains(event.target))
      if (pressing) window.setTimeout(() => (pressing = false), 0)
    }
    const onFocusOut = (event: FocusEvent) => {
      if (pressing && lastFocus.current?.element === event.target)
        lastFocus.current = null
    }
    const removals = new MutationObserver(() => giveBack.current())
    removals.observe(root, { childList: true, subtree: true })
    const onReturn = () => giveBack.current()
    document.addEventListener("focusin", onFocusIn)
    document.addEventListener("focusout", onFocusOut)
    document.addEventListener("mousedown", onPress, true)
    window.addEventListener("focus", onReturn)
    return () => {
      removals.disconnect()
      document.removeEventListener("focusin", onFocusIn)
      document.removeEventListener("focusout", onFocusOut)
      document.removeEventListener("mousedown", onPress, true)
      window.removeEventListener("focus", onReturn)
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
        <div
          ref={surface}
          className="agents-overview-surface"
          data-list-width={listWidth === null ? undefined : true}
          style={
            {
              "--agents-list-min": `${listMin}px`,
              "--agents-peek-min": `${peekMin}px`,
              ...(listWidth === null ? {} : { "--agents-list-width": `${listWidth}px` }),
            } as CSSProperties
          }
        >
          {/* The header stays where it is; only the list under it scrolls. */}
          <div ref={side} className="agents-overview-side">
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
            {split && peeked !== null ? (
              <ResizeEdge
                label="Resize Agents List"
                className="agents-overview-edge"
                value={{ now: drawn.list, min: listMin, max: listMax(drawn.surface) }}
                onStart={() => {
                  dragFrom.current = {
                    list: side.current?.getBoundingClientRect().width ?? drawn.list,
                    surface: surface.current?.getBoundingClientRect().width ?? 0,
                  }
                }}
                onMove={(delta) => {
                  const { list: from, surface: room } = dragFrom.current
                  onListWidth(
                    Math.round(Math.min(Math.max(from + delta, listMin), listMax(room))),
                  )
                }}
                onReset={() => onListWidth(null)}
              />
            ) : null}
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

/**
 * The narrowest the list and the peek beside it are drawn: given to the
 * stylesheet (`--agents-list-min`, `--agents-peek-min`), which holds a
 * dragged width between them as the window resizes, and to the edge.
 */
const listMin = 340
const peekMin = 320
const listMax = (surface: number) => Math.max(listMin, surface - peekMin)

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
 * list on the next frame, as it does any focus it loses.
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
        <EmptyState
          variant="compact"
          className="agents-overview-resting"
          data-reflow="empty"
          title={emptyGroup[quiet]}
        />
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
        <p className="agents-overview-footnote" data-reflow="hidden">
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
