import { historyNotices } from "../../../../conversation"
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
import { retryTranscript } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectOutbox,
  selectSession,
  selectTranscript,
  selectTranscriptFailure,
} from "../../adapters/store/selectors"
import { ProviderSignIn } from "./provider-sign-in"
import { ApprovalCard } from "./approval-card"
import { LiveRow } from "./live-row"
import { Message } from "./message"
import { QuestionCard } from "./question-card"
import { type Message as MessageValue } from "../../model/transcript"
import { TranscriptHeading } from "./transcript-heading"
import "./transcript.css"
import { readFailureCopy } from "../failure-copy"

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
 * there; scrolled up to read, they stay put.
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
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const outbox = useWorkspaceSelector((state) => selectOutbox(state, sessionId))
  const failure = useWorkspaceSelector((state) =>
    selectTranscriptFailure(state, sessionId),
  )
  const model = useWorkspaceSelector((state) => selectSession(state, sessionId)?.model)
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
  // read — stay put too.
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
    const observer = new ResizeObserver(() => {
      if (!active) return
      if (pinned.current) scroller.scrollTop = scroller.scrollHeight
      measure()
    })
    observer.observe(scroller)
    if (scroller.firstElementChild) observer.observe(scroller.firstElementChild)
    scroller.addEventListener("scroll", onScroll, { passive: true })
    return () => {
      active = false
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
  const messages = useMemo(() => {
    const combined = outbox.length === 0 ? held : [...held, ...outbox]
    if (refusalId === undefined) return combined
    return combined.map((message) => shownWhileRecovery(message, refusalId))
  }, [held, outbox, refusalId])
  if (stayPut.current === null && transcript) {
    // Initial content and messages already shown while the read was pending
    // stay put. Later additions have their own entrance.
    stayPut.current = new Set(
      [...shownBefore.current, ...messages].map((message) => message.id),
    )
  }
  // What is on screen, recorded once committed: a render React lets go shows nothing.
  useLayoutEffect(() => {
    shownBefore.current = messages
  }, [messages])
  const loaded = transcript !== undefined

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
          {historyNotices(transcript?.history).map((notice) => (
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
          {transcript?.activity ? <LiveRow activity={transcript.activity} /> : null}
          {(transcript?.questions ?? []).map((question) => (
            <QuestionCard
              key={JSON.stringify([question.executionId, question.questionId])}
              sessionId={sessionId}
              question={question}
            />
          ))}
          {transcript?.approval && model ? (
            <ApprovalCard
              key={transcript.approval.id}
              sessionId={sessionId}
              approval={transcript.approval}
              model={model}
            />
          ) : null}
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
