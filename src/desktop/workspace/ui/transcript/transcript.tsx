import { offersAuthenticationRecovery } from "../../../../provider-authentication/model/recovery"
import { memo, useEffect, useLayoutEffect, useMemo, useRef, type RefObject } from "react"
import { retryTranscript } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectOutbox,
  selectSession,
  selectTranscript,
  selectTranscriptFailure,
} from "../../adapters/store/selectors"
import { reducedMotion } from "../../../adapters/motion-preference"
import { ProviderSignIn } from "./provider-sign-in"
import { ApprovalCard } from "./approval-card"
import { LiveRow } from "./live-row"
import { Message } from "./message"
import type { Message as MessageValue } from "../../model/transcript"
import { TranscriptHeading } from "./transcript-heading"
import "./transcript.css"
import { readFailureCopy } from "../failure-copy"

const noMessages: readonly MessageValue[] = []

/** How close to the end the reader must be for a growing reply to keep them there. */
const pinnedWithin = 24

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
  arriving,
  scrollRef,
  headingRef,
  onHeadingVisible,
}: {
  sessionId: string
  /** Set when this conversation replaces the home that sent its first message. */
  arriving: boolean
  scrollRef: RefObject<HTMLDivElement | null>
  headingRef: RefObject<HTMLDivElement | null>
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
  const provider = transcript?.agent
  const titleRef = useRef<HTMLHeadingElement>(null)
  const pinned = useRef(true)
  // Messages there when the conversation first showed stay put; later ones rise
  // into place. Messages already on screen before it loaded — sent while it was
  // read — stay put too; an arriving conversation's first message has its own motion.
  const shownBefore = useRef<readonly MessageValue[]>(noMessages)
  // Set once, when the conversation first loads; the same on any render after.
  const stayPut = useRef<ReadonlySet<string> | null>(null)
  const report = useRef(onHeadingVisible)
  report.current = onHeadingVisible

  // The header shows the title only once the heading's own has scrolled away.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const title = titleRef.current
    if (!scroller || !title) return
    const top = scroller.getBoundingClientRect().top
    report.current(title.getBoundingClientRect().bottom > top + 4)
    const observer = new IntersectionObserver(
      ([entry]) => report.current(entry.isIntersecting),
      { root: scroller, rootMargin: "-4px 0px 0px 0px" },
    )
    observer.observe(title)
    return () => observer.disconnect()
  }, [scrollRef])

  useEffect(() => {
    const scroller = scrollRef.current
    if (!scroller) return
    const onScroll = () => {
      pinned.current =
        scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < pinnedWithin
    }
    const observer = new ResizeObserver(() => {
      if (pinned.current) scroller.scrollTop = scroller.scrollHeight
    })
    observer.observe(scroller)
    if (scroller.firstElementChild) observer.observe(scroller.firstElementChild)
    scroller.addEventListener("scroll", onScroll, { passive: true })
    return () => {
      observer.disconnect()
      scroller.removeEventListener("scroll", onScroll)
    }
  }, [scrollRef])

  // The source's conversation, then what the person sent that it does not hold yet.
  const held = transcript?.messages ?? noMessages
  const messages = useMemo(
    () => (outbox.length === 0 ? held : [...held, ...outbox]),
    [held, outbox],
  )
  const count = messages.length
  if (stayPut.current === null && transcript) {
    // Held still: all that is there when it loads — or, arriving, what was
    // already on screen and the first message, which has its own motion.
    const still = arriving ? [...shownBefore.current, ...messages.slice(0, 1)] : messages
    stayPut.current = new Set(still.map((message) => message.id))
  }
  // What is on screen, recorded once committed: a render React lets go shows nothing.
  useLayoutEffect(() => {
    shownBefore.current = messages
  }, [messages])
  const loaded = transcript !== undefined
  // Opening lands on the latest message, before paint; an arrival holds still under its motion.
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    if (!scroller || !loaded || arriving) return
    scroller.scrollTop = scroller.scrollHeight
    pinned.current = true
  }, [loaded, arriving, scrollRef])

  // A new message glides into view for a reader at the end; growth keeps them pinned.
  const previousCount = useRef(count)
  useLayoutEffect(() => {
    const scroller = scrollRef.current
    const grew = count > previousCount.current
    previousCount.current = count
    if (!scroller || !grew || !pinned.current) return
    scroller.scrollTo({
      top: scroller.scrollHeight,
      behavior: reducedMotion() ? "auto" : "smooth",
    })
  }, [count, scrollRef])

  return (
    <div
      className="workspace-transcript"
      ref={scrollRef}
      tabIndex={-1}
      data-pane-focus
      // One screen of it is what a drag's copy of the pane shows.
      data-split-scroll
    >
      <div className="workspace-transcript-inner" data-arriving={arriving || undefined}>
        <TranscriptHeading ref={headingRef} sessionId={sessionId} titleRef={titleRef} />
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
        {messages.map((message) => (
          <Message
            key={message.id}
            sessionId={sessionId}
            message={message}
            isNew={stayPut.current !== null && !stayPut.current.has(message.id)}
          />
        ))}
        {offersAuthenticationRecovery(
          transcript?.authenticationRefusal,
          transcript?.latestInputId,
          outbox,
        ) &&
        (provider === "claude" || provider === "codex") ? (
          <ProviderSignIn key={provider} provider={provider} />
        ) : null}
        {transcript?.activity ? <LiveRow activity={transcript.activity} /> : null}
        {transcript?.approval && model ? (
          <ApprovalCard
            sessionId={sessionId}
            approval={transcript.approval}
            model={model}
          />
        ) : null}
      </div>
    </div>
  )
})
