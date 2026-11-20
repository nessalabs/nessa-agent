/**
 * Auto-pick on a waiting decision: when the gateway sends its recommended
 * choice for the person, with a button that holds it ("Wait for me") and one
 * that lets it go again; and, once it picked, "Change my answer". The
 * gateway owns the deadline and the pick (`conversation.decisions`,
 * `conversation.pauseAutoPick`); this only says it and asks for a pause.
 */
import { useEffect, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import { useNow } from "../../../../clock"
import { left, type Decision } from "../../model/decisions"
import { sendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { DesktopIcon } from "../../../ui/icons"
import { useDecisionSource, useDecisions } from "./decisions-source"
import "./decisions.css"

/** The line under a waiting ask's choices, when auto-pick will pick it or is paused. */
export function AutoPickLine({ sessionId, askId }: { sessionId: string; askId: string }) {
  const read = useDecisions(sessionId)
  const source = useDecisionSource()
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)
  if (read.state === "none" || !source) return null
  const decision = read.decisions?.find((each) => each.ask.askId === askId)
  const pick = decision?.autoPick
  if (!decision || decision.outcome.state !== "waiting" || !pick) return null
  const label = decision.ask.choices[decision.ask.recommended]?.label ?? ""
  const hold = (paused: boolean) => {
    setBusy(true)
    setFailed(false)
    source.pauseAutoPick(sessionId, askId, paused).then(
      () => {
        setBusy(false)
        read.retry()
      },
      (error: unknown) => {
        console.error("Pausing auto-pick failed", error)
        setBusy(false)
        setFailed(true)
      },
    )
  }
  return (
    <p
      className="auto-pick"
      data-auto-pick-line={pick.state}
      aria-busy={busy || undefined}
    >
      <DesktopIcon name="timer" size={14} aria-hidden="true" />
      {pick.state === "due" ? (
        <Countdown label={label} atMs={pick.atMs} onLate={read.retry} />
      ) : (
        <span>Auto-pick paused</span>
      )}
      <Button
        variant="ghost"
        size="icon-sm"
        className="auto-pick-button"
        disabled={busy}
        aria-label={pick.state === "due" ? "Wait for me" : "Resume auto-pick"}
        title={pick.state === "due" ? "Wait for me" : "Resume auto-pick"}
        data-auto-pick-hold={pick.state === "due" ? "pause" : "resume"}
        onClick={() => hold(pick.state === "due")}
      >
        <DesktopIcon
          name={pick.state === "due" ? "pause" : "resume"}
          size={14}
          aria-hidden="true"
        />
      </Button>
      {failed ? (
        <span role="status" className="auto-pick-failed">
          Didn’t go through. Try again.
        </span>
      ) : null}
    </p>
  )
}

/**
 * How long a due pick may stay unsent on screen before the decisions are
 * read again: the gateway may have picked, or the person's settings or a
 * pause from elsewhere moved it.
 */
const LATE_MS = 5000

/** Ticks each second only while there is a pick to count down to. */
function Countdown({
  label,
  atMs,
  onLate,
}: {
  label: string
  atMs: number
  onLate: () => void
}) {
  const now = useNow(1000)
  const late = now >= atMs + LATE_MS
  useEffect(() => {
    if (late) onLate()
  }, [late, onLate])
  if (now >= atMs) return <span>Picking {label}…</span>
  return (
    <span>
      Picks {label} in{" "}
      <b className="auto-pick-clock" data-auto-pick-clock="">
        {left(atMs, now)}
      </b>
    </span>
  )
}

/**
 * Under an auto-picked decision: "Change my answer" opens the other choices,
 * and a tapped one goes as the person's own message, which the agent reads
 * after the pick.
 */
export function ChangeAnswer({
  sessionId,
  decision,
}: {
  sessionId: string
  decision: Decision
}) {
  const dispatch = useWorkspaceDispatch()
  const read = useDecisions(sessionId)
  const [open, setOpen] = useState(false)
  const [sent, setSent] = useState<string | null>(null)
  const [sending, setSending] = useState(false)
  const [failed, setFailed] = useState(false)
  const outcome = decision.outcome
  if (outcome.state !== "decided" || !outcome.auto) return null
  // Any message from the person answers the ask waiting now, so with a newer
  // one waiting a change would be taken for its answer: answer that instead.
  const newer =
    read.state !== "none" &&
    read.decisions?.some(
      (each) => each.outcome.state === "waiting" && each.ask.askId !== decision.ask.askId,
    )
  if (newer && sent === null) return null
  if (sent !== null)
    return (
      <p className="auto-pick-changed" role="status" data-change-answer-sent={sent}>
        Sent: {sent}
      </p>
    )
  if (!open)
    return (
      <Button
        variant="outline"
        size="sm"
        className="auto-pick-change"
        data-change-answer=""
        onClick={() => setOpen(true)}
      >
        Change my answer
      </Button>
    )
  const choose = (label: string) => {
    setSending(true)
    setFailed(false)
    const text = `${label}. Changed from the auto-pick on “${decision.ask.question}”`
    void dispatch(sendMessage({ sessionId, text, initiator: "person" })).then(
      (result) => {
        setSending(false)
        if (result === "sent") setSent(label)
        else setFailed(true)
      },
    )
  }
  return (
    <div className="auto-pick-choices" data-change-answer-choices="">
      {decision.ask.choices.map((choice, index) =>
        index === outcome.choice ? null : (
          <Button
            key={choice.label}
            variant="outline"
            size="sm"
            disabled={sending}
            data-change-answer-choice={choice.label}
            onClick={() => choose(choice.label)}
          >
            {choice.label}
          </Button>
        ),
      )}
      <Button variant="ghost" size="sm" disabled={sending} onClick={() => setOpen(false)}>
        Keep it
      </Button>
      {failed ? (
        <span role="status" className="auto-pick-failed">
          Not sent. Pick again, or write your answer.
        </span>
      ) : null}
    </div>
  )
}
