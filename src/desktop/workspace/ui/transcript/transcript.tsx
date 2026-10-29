import { historyNotices, UsageLimitCard } from "../../../../conversation"
import {
  offersAuthenticationRecovery,
  signInProvider,
} from "../../../../provider-authentication/model/recovery"
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react"
import { curveToken, durationToken } from "../../../adapters/motion"
import { IconButton } from "../../../ui/icon-button"
import {
  loadEarlier,
  retryTranscript,
  setUsageLimitAutoContinue,
} from "../../adapters/store/commands"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectOutbox, selectTranscriptFailure } from "../../adapters/store/selectors"
import {
  useRememberedSession,
  useRememberedTranscript,
  useRememberedValue,
} from "../../adapters/store/remembered-session"
import { ProviderSignIn } from "./provider-sign-in"
import { ApprovalCard } from "./approval-card"
import { LiveRow } from "./live-row"
import { Message } from "./message"
import { QuestionCard } from "./question-card"
import { type Message as MessageValue } from "../../model/transcript"
import { TranscriptHeading } from "./transcript-heading"
import "./transcript.css"
import { readFailureCopy } from "../failure-copy"
import { followTranscriptEnd, watchToolDisclosure } from "./tool-disclosure"

const noMessages: readonly MessageValue[] = []

/**
 * A pure authentication refusal keeps its mark on the transcript. The pane
 * draws that mark only once the sign-in card is gone — a local retry hides
 * the card without a new gateway view. An independent required-work failure
 * stays beside the card.
 */
function shownWhileRecovery(message: MessageValue, refusalId: string): MessageValue {
  if (message.role === "note" || !message.terminal || message.requiredWorkFailed === true)
    return message
  if (message.executionId !== refusalId) return message
  return { ...message, terminal: undefined }
}

/**
 * A pure usage limit keeps its mark on the transcript. The pane draws that
 * mark only when independent required work also failed. The card is the
 * limit's own notice.
 */
function shownWhileUsageLimit(message: MessageValue, executionId: string): MessageValue {
  if (message.role === "note" || !message.terminal || message.requiredWorkFailed === true)
    return message
  if (message.executionId !== executionId) return message
  return { ...message, terminal: undefined }
}

function latestTurn(messages: readonly MessageValue[]) {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index]
    if (message && (message.role === "user" || message.role === "agent")) return message
  }
  return undefined
}

/**
 * How far above the screen's top the earlier turns start loading, so a reader
 * scrolling back meets them already there.
 */
const earlierWithin = "1200px"

/** How long after a failed read of earlier turns the place asks again. */
const earlierRetryMs = 2_000

/** Where `element` sits in `scroller`'s content, whatever the scroll. */
const contentTop = (scroller: HTMLElement, element: Element) =>
  element.getBoundingClientRect().top -
  scroller.getBoundingClientRect().top +
  scroller.scrollTop

/**
 * What stays put while earlier turns load above the reader: the first
 * message they see, held by its top. The first message drawn may start
 * partway through its turn, and a page lengthens it from above: that one is
 * held by its bottom, which a page above does not move.
 */
interface Anchor {
  readonly element: Element
  readonly edge: "top" | "bottom"
  at: number
}

const edgeOf = (scroller: HTMLElement, element: Element, edge: Anchor["edge"]) =>
  contentTop(scroller, element) +
  (edge === "bottom" ? element.getBoundingClientRect().height : 0)

function atTop(scroller: HTMLElement): Anchor | null {
  const top = scroller.getBoundingClientRect().top
  const messages = [...scroller.querySelectorAll(".workspace-message")]
  const element = messages.find((message) => message.getBoundingClientRect().bottom > top)
  if (!element) return null
  const edge = element === messages[0] ? "bottom" : "top"
  return { element, edge, at: edgeOf(scroller, element, edge) }
}

/** How close to the end the reader must be for a growing reply to keep them there. */
const pinnedWithin = 24

/**
 * How far from the end, as a share of the screen, the reader must be before
 * the way back to the latest is offered: a nudge past the end is not away.
 */
const awayScreens = 0.5

/** How far `scroller` is from showing its end, in pixels. */
const distanceFromEnd = (scroller: HTMLElement) =>
  scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight

/**
 * A session's conversation, read top to bottom: its heading, its messages,
 * what the agent is doing, and what it waits on. It subscribes to this one
 * session's transcript only, so a reply streaming into another pane never
 * renders it.
 *
 * Opening lands on the latest message. While the reader is at the end, a
 * growing reply, a new message, or a pane resized by a split keeps them
 * there; scrolled up to read, they stay put. A tool they open or close keeps
 * the clicked header where it is and opens downward (`tool-disclosure.ts`).
 * Scrolled back near the top of a long conversation, its earlier turns load
 * above what is shown, and what the reader was reading stays where it was.
 */
export const Transcript = memo(function Transcript({
  sessionId,
  scrollRef,
  onHeadingVisible,
}: {
  sessionId: string
  /** Set when this conversation replaces the home that sent its first message. */
  scrollRef: RefObject<HTMLDivElement | null>
  /** Reports whether the heading's title is in view, so the pane's header need not repeat it. */
  onHeadingVisible: (visible: boolean) => void
}) {
  const dispatch = useWorkspaceDispatch()
  const transcript = useRememberedTranscript(sessionId)
  const liveOutbox = useWorkspaceSelector((state) => selectOutbox(state, sessionId))
  const outbox = useRememberedValue(sessionId, liveOutbox)
  const liveFailure = useWorkspaceSelector((state) =>
    selectTranscriptFailure(state, sessionId),
  )
  const failure = useRememberedValue(sessionId, liveFailure)
  const model = useRememberedSession(sessionId)?.model
  const provider = signInProvider(transcript?.agent)
  const recoveryShown =
    provider !== undefined &&
    offersAuthenticationRecovery(
      transcript?.authenticationRefusal,
      transcript?.latestInputId,
      outbox,
    )
  const titleRef = useRef<HTMLHeadingElement>(null)
  const pinned = useRef(true)
  // Messages there when the conversation first showed stay put; later ones rise
  // into place. Messages already on screen before it loaded — sent while it was
  // read — stay put too, and so do earlier turns loaded above them.
  const shownBefore = useRef<readonly MessageValue[]>(noMessages)
  // Set once, when the conversation first loads; the same on any render after.
  const stayPut = useRef<ReadonlySet<string> | null>(null)
  // Whether the reader is far enough from the end to be offered the way back.
  const [away, setAway] = useState(false)
  const jump = useRef<HTMLButtonElement>(null)
  const report = useRef(onHeadingVisible)
  report.current = onHeadingVisible

  // The header shows the title only once the heading's own has scrolled away.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const title = titleRef.current
    if (!scroller || !title) return
    const observer = new IntersectionObserver(
      ([entry]) => report.current(entry.isIntersecting),
      { root: scroller, rootMargin: "-4px 0px 0px 0px" },
    )
    observer.observe(title)
    return () => observer.disconnect()
  }, [scrollRef])

  useLayoutEffect(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    let active = true
    const measure = () => {
      const distance = distanceFromEnd(scroller)
      setAway(distance > scroller.clientHeight * awayScreens)
      return distance
    }
    const onScroll = () => {
      pinned.current = measure() < pinnedWithin
    }
    // A tool disclosure holds the end still for its own motion, then the
    // distance decides whether a later growth follows (`tool-disclosure.ts`).
    const stopWatchingDisclosure = watchToolDisclosure(scroller, () => {
      if (!active) return
      pinned.current = measure() < pinnedWithin
    })
    const observer = new ResizeObserver(() => {
      if (!active) return
      followTranscriptEnd(scroller, pinned.current)
      measure()
    })
    observer.observe(scroller)
    if (scroller.firstElementChild) observer.observe(scroller.firstElementChild)
    scroller.addEventListener("scroll", onScroll, { passive: true })
    return () => {
      active = false
      stopWatchingDisclosure()
      observer.disconnect()
      scroller.removeEventListener("scroll", onScroll)
    }
  }, [scrollRef])

  // Back at the end the button goes; the keyboard stays in the conversation, not on nothing.
  useEffect(() => {
    const scroller = scrollRef.current
    if (away || !scroller) return
    // This transcript's own button: another pane's keeps its focus.
    if (jump.current && scroller.ownerDocument.activeElement === jump.current)
      scroller.focus({ preventScroll: true })
  }, [away, scrollRef])

  // The return under way, if any: cancelling it leaves the reader where it got to.
  const returning = useRef<(() => void) | null>(null)
  useEffect(() => () => returning.current?.(), [])
  const returnToLatest = useCallback(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    returning.current?.()
    const settle = () => {
      returning.current = null
      pinned.current = true
    }
    // The stylesheet's duration, zero under reduced motion (`adapters/motion.ts`).
    const duration = durationToken(scroller, "--desktop-slow")
    if (duration === 0) {
      scroller.scrollTop = scroller.scrollHeight
      settle()
      return
    }
    const view = scroller.ownerDocument.defaultView ?? window
    // The stylesheet's curve too, so the return moves as the window does.
    const ease = curveToken(scroller, "--desktop-out")
    const from = scroller.scrollTop
    const started = view.performance.now()
    let frame = 0
    // The reader taking the scroll back ends the return where it is.
    const interrupts = ["wheel", "touchstart", "pointerdown", "keydown"] as const
    const cancel = () => {
      view.cancelAnimationFrame(frame)
      for (const type of interrupts) scroller.removeEventListener(type, cancel)
      if (returning.current === cancel) returning.current = null
    }
    const step = (now: number) => {
      const progress = Math.min(1, (now - started) / duration)
      // Aimed at the end as it is now, so a reply growing meanwhile is caught.
      const end = scroller.scrollHeight - scroller.clientHeight
      scroller.scrollTop = from + (end - from) * ease(progress)
      if (progress < 1) frame = view.requestAnimationFrame(step)
      else {
        cancel()
        settle()
      }
    }
    for (const type of interrupts)
      scroller.addEventListener(type, cancel, { passive: true })
    returning.current = cancel
    frame = view.requestAnimationFrame(step)
  }, [scrollRef])

  // The source's conversation, then what the person sent that it does not hold yet.
  const held = transcript?.messages ?? noMessages
  const refusalId = recoveryShown ? transcript?.authenticationRefusal : undefined
  // A message still on its way is the latest turn, so the card follows it.
  const usageLimitTurn = latestTurn(outbox.length > 0 ? outbox : held)
  const usageLimit = usageLimitTurn?.usageLimit === undefined ? undefined : usageLimitTurn
  const messages = useMemo(() => {
    const combined = outbox.length === 0 ? held : [...held, ...outbox]
    const recovered =
      refusalId === undefined
        ? combined
        : combined.map((message) => shownWhileRecovery(message, refusalId))
    if (usageLimit === undefined || usageLimit.requiredWorkFailed === true)
      return recovered
    return recovered.map((message) =>
      shownWhileUsageLimit(message, usageLimit.executionId ?? ""),
    )
  }, [held, outbox, refusalId, usageLimit])
  if (stayPut.current === null && transcript) {
    // Initial content and messages already shown while the read was pending
    // stay put. Later additions have their own entrance.
    stayPut.current = new Set(
      [...shownBefore.current, ...messages].map((message) => message.id),
    )
  } else if (stayPut.current !== null) {
    // Earlier turns loaded above the first one shown are not new.
    const oldest = shownBefore.current[0]?.id
    const at = messages.findIndex((message) => message.id === oldest)
    for (const message of messages.slice(0, Math.max(at, 0)))
      (stayPut.current as Set<string>).add(message.id)
  }
  // What is on screen, recorded once committed: a render React lets go shows nothing.
  useLayoutEffect(() => {
    shownBefore.current = messages
  }, [messages])
  const loaded = transcript !== undefined
  const earlier = transcript?.history?.earlier === true

  // Near the top with earlier turns to read, they are asked for. Watched
  // again after each page, so a page that left the place still in reach
  // asks for the next.
  const earlierRef = useRef<HTMLDivElement>(null)
  const anchor = useRef<Anchor | null>(null)
  const [pagesRead, setPagesRead] = useState(0)
  const retryEarlier = useRef<ReturnType<typeof setTimeout>>(undefined)
  useEffect(() => {
    const scroller = scrollRef.current
    const place = earlierRef.current
    if (!earlier || !scroller || !place) return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return
        // What the reader is looking at is held in place while the read is
        // under way: only then is layout read, never on an ordinary render.
        const held = anchor.current ?? atTop(scroller)
        anchor.current = held
        void dispatch(loadEarlier({ sessionId })).then((read) =>
          requestAnimationFrame(() => {
            if (anchor.current === held) anchor.current = null
            // Watched again: a page that left this place in reach asks for
            // the next, though it only lengthened the first turn shown. A
            // read that failed is asked again a moment on.
            if (read) setPagesRead((count) => count + 1)
            else
              retryEarlier.current = setTimeout(
                () => setPagesRead((count) => count + 1),
                earlierRetryMs,
              )
          }),
        )
      },
      { root: scroller, rootMargin: `${earlierWithin} 0px 0px 0px` },
    )
    observer.observe(place)
    return () => {
      observer.disconnect()
      clearTimeout(retryEarlier.current)
    }
  }, [dispatch, earlier, pagesRead, scrollRef, sessionId])

  // Turns loaded above keep the reader on what they were reading: while a
  // read is under way, what was at the top of the screen stays there
  // (`atTop`). At the end, the end is followed instead.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const held = anchor.current
    if (!scroller || !held) return
    if (!held.element.isConnected || pinned.current) return
    const at = edgeOf(scroller, held.element, held.edge)
    if (at !== held.at) scroller.scrollTop += at - held.at
    held.at = at
  }, [messages, earlier, scrollRef])

  return (
    // The way back to the latest floats over the scroller's foot, above the composer.
    <div className="workspace-transcript-frame" data-split-through>
      <div
        className="workspace-transcript"
        ref={scrollRef}
        tabIndex={-1}
        data-pane-focus
        // The library can picture one screen of this. The workspace's copy
        // leaves the body out (`split-panes-drag.ts`).
        data-split-scroll
      >
        <div className="workspace-transcript-inner">
          <TranscriptHeading
            sessionId={sessionId}
            titleRef={titleRef}
            lease={transcript?.lease}
          />
          {failure && !loaded ? (
            <div className="workspace-transcript-note" role="status">
              <p>{readFailureCopy(failure, "conversation")}</p>
              <button
                type="button"
                className="workspace-button"
                onClick={() => dispatch(retryTranscript({ sessionId }))}
              >
                Try Again
              </button>
            </div>
          ) : null}
          {earlier ? (
            <div
              ref={earlierRef}
              className="workspace-transcript-note"
              data-history-earlier=""
            >
              <LoadingText />
            </div>
          ) : null}
          {historyNotices(transcript?.history, { pagesEarlier: true }).map((notice) => (
            <div
              key={notice.kind}
              className="workspace-transcript-note"
              role="status"
              data-history-extent={notice.kind}
            >
              <p>{notice.text}</p>
            </div>
          ))}
          {messages.map((message) => (
            <Message
              key={message.id}
              sessionId={sessionId}
              message={message}
              isNew={stayPut.current !== null && !stayPut.current.has(message.id)}
            />
          ))}
          {usageLimit?.usageLimit ? (
            <UsageLimitCard
              limit={usageLimit.usageLimit}
              enabled={transcript?.usageLimitAutoContinue === true}
              onChange={(enabled) =>
                dispatch(setUsageLimitAutoContinue({ enabled, initiator: "person" }))
              }
            />
          ) : null}
          {recoveryShown && provider ? (
            <ProviderSignIn key={provider} provider={provider} />
          ) : null}
          {transcript?.interactionNotice ? (
            <p
              className="workspace-transcript-notice"
              role="status"
              data-interaction-notice
            >
              {transcript.interactionNotice}
            </p>
          ) : null}
          <LiveRow activity={transcript?.activity ?? null} after={messages} />
          {(transcript?.questions ?? []).map((question) => (
            <QuestionCard
              key={JSON.stringify([
                question.key ?? null,
                question.executionId,
                question.questionId,
              ])}
              sessionId={sessionId}
              question={question}
            />
          ))}
          {/* One card for each review, in the order the source lists them:
            each loads, retries and is answered on its own. */}
          {model
            ? transcript?.approvals.map((approval) => (
                <ApprovalCard
                  key={approval.id}
                  sessionId={sessionId}
                  approval={approval}
                  model={model}
                />
              ))
            : null}
        </div>
      </div>
      <IconButton
        ref={jump}
        icon="chevronDown"
        label="Jump to Latest"
        size="lg"
        shape="pill"
        className="workspace-jump"
        tooltipSide="above"
        data-hidden={away ? undefined : ""}
        aria-hidden={away ? undefined : true}
        tabIndex={away ? undefined : -1}
        onClick={returnToLatest}
      />
    </div>
  )
})
