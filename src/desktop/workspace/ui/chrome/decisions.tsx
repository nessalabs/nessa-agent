/**
 * Decisions in Chat context: the latest three a chat's agents asked the
 * person, waiting first, and "See all", which opens every decision in the
 * chat in a dialog: the list, filtered To decide · All · Decided, beside the
 * chosen decision in full (the expanded card). A row opens the dialog on
 * that decision. The chat's waiting decision is answered there, with an
 * optional note, and the answer is still the person's own message.
 */
import { useEffect, useState } from "react"
import { Badge } from "@nessa-ui/react/badge"
import { Button } from "@nessa-ui/react/button"
import { Dialog } from "@nessa-ui/react/dialog"
import { Input } from "@nessa-ui/react/input"
import { Kbd } from "@nessa-ui/react/kbd"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { useNow } from "../../../../clock"
import {
  ago,
  answerLabel,
  decisionLine,
  decisionsFiltered,
  decisionsInOrder,
  decisionsShown,
  filterFor,
  stateLabel,
  type Decision,
  type DecisionsFilter,
} from "../../model/decisions"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"
import { undoLabels } from "../../../../conversation"
import { openSettings } from "../../../settings"
import { DesktopIcon } from "../../../ui/icons"
import { sendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import { AutoPickLine, ChangeAnswer } from "./auto-pick"
import type { DecisionsRead } from "./decisions-source"
import "./decisions.css"

/** The rows under the Decisions header, and the dialog they open. */
export function DecisionRows({
  sessionId,
  read,
}: {
  sessionId: string
  read: DecisionsRead
}) {
  const now = useNow(60_000)
  const [opened, setOpened] = useState<{ askId: string | null } | null>(null)
  if (read.state === "none") return null
  const decisions = read.decisions
  // Said beside the rows it could not refresh, or in their place before any.
  const failed =
    read.state === "failed" ? (
      <p className="chat-context-none" data-decisions-failed="">
        {decisions ? "Could not refresh" : "Decisions could not be read"}
        {" · "}
        <Button
          variant="link"
          size="sm"
          className="chat-context-retry"
          onClick={read.retry}
        >
          Try again
        </Button>
      </p>
    ) : null
  if (!decisions) return failed ?? <p className="chat-context-none">Reading decisions</p>
  const ordered = decisionsInOrder(decisions)
  if (ordered.length === 0)
    return failed ?? <p className="chat-context-none">Nothing asked yet</p>
  const shown = ordered.slice(0, decisionsShown)
  const more = ordered.length - shown.length
  return (
    <>
      <ul className="chat-context-rows chat-context-decisions" aria-label="Decisions">
        {shown.map((decision) => (
          <li key={decision.ask.askId}>
            <button
              type="button"
              className="chat-context-row chat-context-decision"
              data-decision={decision.ask.askId}
              data-state={decision.outcome.state}
              onClick={() => setOpened({ askId: decision.ask.askId })}
            >
              <span className="decisions-dot" data-state={decision.outcome.state} />
              <span className="chat-context-name">{decision.ask.question}</span>
              <span className="chat-context-line">{decisionLine(decision, now)}</span>
            </button>
          </li>
        ))}
      </ul>
      <Button
        variant="ghost"
        size="sm"
        className="chat-context-more"
        data-decisions-all=""
        onClick={() => setOpened({ askId: null })}
      >
        {more > 0 ? `${more} more · See all` : "See all"}
      </Button>
      {failed}
      {opened ? (
        <DecisionsDialog
          sessionId={sessionId}
          decisions={ordered}
          openOn={opened.askId ?? ordered[0]?.ask.askId ?? null}
          now={now}
          onClose={() => setOpened(null)}
        />
      ) : null}
    </>
  )
}

const filters: readonly { value: DecisionsFilter; label: string }[] = [
  { value: "waiting", label: "To decide" },
  { value: "all", label: "All" },
  { value: "decided", label: "Decided" },
]

const effortWords = {
  small: "Small effort",
  medium: "Medium effort",
  large: "Large effort",
}

/**
 * Every decision in the chat: the list, filtered, beside the chosen one in
 * full. It opens on the one chosen, on To decide when that one waits and on
 * All otherwise. While the chosen one is the chat's waiting ask, the answer
 * bar sits under it: auto-pick, a note, and Answer.
 */
function DecisionsDialog({
  sessionId,
  decisions,
  openOn,
  now,
  onClose,
}: {
  sessionId: string
  decisions: readonly Decision[]
  openOn: string | null
  now: number
  onClose: () => void
}) {
  const title = useWorkspaceSelector((state) => selectSession(state, sessionId)?.title)
  const liveAsk = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.ask?.askId,
  )
  const dispatch = useWorkspaceDispatch()
  const [selected, setSelected] = useState(openOn)
  const [filter, setFilter] = useState<DecisionsFilter>(() =>
    filterFor(decisions.find((decision) => decision.ask.askId === openOn)),
  )
  const [picked, setPicked] = useState<number | null>(null)
  const [note, setNote] = useState("")
  const [sending, setSending] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const listed = decisionsFiltered(decisions, filter)
  const decision = decisions.find((each) => each.ask.askId === selected)
  const live =
    decision?.outcome.state === "waiting" && decision.ask.askId === liveAsk
      ? decision
      : undefined
  // A pick is for one decision: choosing another starts again.
  useEffect(() => {
    setPicked(null)
    setNote("")
    setFailure(null)
  }, [selected, liveAsk])
  const pick = (index: number) => {
    if (live && !sending && index < live.ask.choices.length) setPicked(index)
  }
  // 1–4 picks a choice wherever focus is in the dialog, unless typing.
  const choices = live?.ask.choices.length ?? 0
  useEffect(() => {
    if (!choices || sending) return
    const onKey = (event: KeyboardEvent) => {
      const target = event.target
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
        return
      if (event.metaKey || event.ctrlKey || event.altKey) return
      if (!/^[1-4]$/.test(event.key)) return
      const index = Number(event.key) - 1
      if (index < choices) setPicked(index)
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [choices, sending])
  const answer = () => {
    const choice = live && picked !== null ? live.ask.choices[picked] : undefined
    if (!choice || sending) return
    const said = note.trim()
    // The label, then ". " and the note: the gateway reads it as that choice.
    const text = said ? `${choice.label}. ${said}` : choice.label
    setSending(true)
    setFailure(null)
    void dispatch(sendMessage({ sessionId, text, initiator: "person" })).then(
      (outcome) => {
        setSending(false)
        if (outcome !== "sent")
          setFailure("Not sent. Answer again, or write your answer in the chat.")
      },
    )
  }
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
      size="xl"
      title="Decisions"
      description={title}
      data-decisions-dialog=""
      footer={
        live ? (
          <div className="decisions-answer" data-decisions-answer="">
            <AutoPickLine sessionId={sessionId} askId={live.ask.askId} />
            <Input
              className="decisions-answer-note"
              aria-label="Note for the agent"
              placeholder="Add a note (optional)"
              value={note}
              disabled={sending}
              data-decisions-note=""
              onChange={(event) => setNote(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                  event.preventDefault()
                  answer()
                }
              }}
            />
            <Button
              disabled={picked === null || sending}
              data-decisions-send=""
              onClick={answer}
            >
              {picked === null
                ? "Pick a choice"
                : `Answer: ${live.ask.choices[picked]?.label ?? ""}`}
            </Button>
            {failure ? (
              <p role="status" className="auto-pick-failed decisions-answer-failure">
                {failure}
              </p>
            ) : null}
          </div>
        ) : undefined
      }
    >
      <div className="decisions-dialog">
        <div className="decisions-dialog-side">
          <SegmentedControl
            aria-label="Show"
            value={filter}
            onValueChange={(value) => {
              const chosen = filters.find((each) => each.value === value)
              if (chosen) setFilter(chosen.value)
            }}
          >
            {filters.map(({ value, label }) => (
              <SegmentedControlOption key={value} value={value} data-filter={value}>
                {label}
              </SegmentedControlOption>
            ))}
          </SegmentedControl>
          {listed.length === 0 ? (
            <p className="chat-context-none">
              {filter === "waiting" ? "Nothing waits on you" : "Nothing decided yet"}
            </p>
          ) : (
            <ul className="decisions-dialog-list" aria-label="Decisions">
              {listed.map((each) => (
                <li key={each.ask.askId} data-decision={each.ask.askId}>
                  <button
                    type="button"
                    className="decisions-dialog-item"
                    aria-current={each.ask.askId === selected || undefined}
                    data-state={each.outcome.state}
                    onClick={() => setSelected(each.ask.askId)}
                  >
                    <span className="decisions-dot" data-state={each.outcome.state} />
                    <span className="decisions-dialog-question">{each.ask.question}</span>
                    <span className="decisions-dialog-line">
                      {decisionLine(each, now)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="decisions-dialog-settings"
            data-decisions-settings=""
            onClick={() => {
              onClose()
              openSettings("decisions")
            }}
          >
            <DesktopIcon name="chatSettings" size={14} aria-hidden="true" />
            Auto-pick settings
          </Button>
        </div>
        {decision ? (
          <DecisionDetail
            sessionId={sessionId}
            decision={decision}
            now={now}
            answering={live !== undefined}
            picked={live ? picked : null}
            onPick={pick}
          />
        ) : (
          <p className="chat-context-none decisions-dialog-empty">
            Pick a decision to see it
          </p>
        )}
      </div>
    </Dialog>
  )
}

/**
 * One decision in full, the expanded card: where it stands, the question
 * and its summary, each choice side by side with what it does, its pros and
 * cons, its effort and how hard it is to undo, and why the agent
 * recommends one. While it is the chat's waiting ask its choices are
 * picked here; once settled, the chosen one is marked and the rest dimmed.
 */
function DecisionDetail({
  sessionId,
  decision,
  now,
  answering,
  picked,
  onPick,
}: {
  sessionId: string
  decision: Decision
  now: number
  answering: boolean
  picked: number | null
  onPick: (index: number) => void
}) {
  const outcome = decision.outcome
  const ask = decision.ask
  const chosen = outcome.state === "decided" ? outcome.choice : undefined
  const recommended = ask.choices[ask.recommended]
  return (
    <article className="decisions-detail" data-decision-detail={ask.askId}>
      <p className="decisions-detail-meta">
        <span className="decisions-dot" data-state={outcome.state} />
        <span data-state={outcome.state} className="decisions-dialog-state">
          {stateLabel(outcome)}
        </span>
        <span>· Asked {ago(decision.askedAtMs, now)}</span>
      </p>
      <h3 className="decisions-detail-question">{ask.question}</h3>
      {ask.summary ? <p className="decisions-detail-summary">{ask.summary}</p> : null}
      {outcome.state === "decided" ? (
        <div
          className="decisions-detail-banner"
          data-auto={outcome.auto || undefined}
          data-decided=""
        >
          <p className="decisions-detail-banner-head">
            {outcome.auto
              ? `Auto-picked: ${answerLabel(decision)}`
              : outcome.choice === undefined
                ? "Decided: Your own words"
                : `Decided: ${answerLabel(decision)}`}
            <span className="decisions-dialog-line"> · {ago(outcome.atMs, now)}</span>
          </p>
          {outcome.auto ? (
            <p className="decisions-dialog-line" data-auto-picked="">
              No answer in time, so the recommended choice was sent.
            </p>
          ) : null}
          {/* What was said, unless it was only the choice's label or the pick's own words. */}
          {!outcome.auto && outcome.text && outcome.text !== answerLabel(decision) ? (
            <blockquote className="decisions-dialog-said">{outcome.text}</blockquote>
          ) : null}
          <ChangeAnswer sessionId={sessionId} decision={decision} />
        </div>
      ) : null}
      {outcome.state === "replaced" ? (
        <div className="decisions-detail-banner" data-replaced="">
          <p className="decisions-detail-banner-head">Not answered</p>
          <p className="decisions-dialog-line">A newer question took its place.</p>
        </div>
      ) : null}
      {outcome.state === "waiting" && !answering ? (
        <p className="decisions-dialog-line">Answer it in the chat.</p>
      ) : null}
      <ul className="decisions-detail-choices" aria-label="Choices">
        {ask.choices.map((choice, index) => {
          const on = answering ? picked === index : chosen === index
          const body = (
            <>
              <span className="decisions-choice-head">
                <span className="decisions-dialog-choice">{choice.label}</span>
                {index === ask.recommended ? (
                  <Badge variant="secondary">Recommended</Badge>
                ) : null}
                {!answering && on ? (
                  <Badge variant="outline">
                    {outcome.state === "decided" && outcome.auto
                      ? "Auto-picked"
                      : "Chosen"}
                  </Badge>
                ) : null}
                {answering ? (
                  <Kbd className="decisions-choice-key">{index + 1}</Kbd>
                ) : null}
              </span>
              <span className="decisions-dialog-line">{choice.consequence}</span>
              {choice.pros?.length || choice.cons?.length ? (
                <span className="decisions-choice-trade">
                  {choice.pros?.map((pro) => (
                    <span key={`+${pro}`} data-pro="">
                      {pro}
                    </span>
                  ))}
                  {choice.cons?.map((con) => (
                    <span key={`-${con}`} data-con="">
                      {con}
                    </span>
                  ))}
                </span>
              ) : null}
              {choice.effort || choice.undo ? (
                <span className="decisions-choice-meta">
                  {choice.effort ? <span>{effortWords[choice.effort]}</span> : null}
                  {choice.undo ? (
                    <span data-undo={choice.undo}>{undoLabels[choice.undo]}</span>
                  ) : null}
                </span>
              ) : null}
            </>
          )
          return (
            <li
              key={choice.label}
              data-chosen={(!answering && on) || undefined}
              data-recommended={index === ask.recommended || undefined}
              data-dim={
                (outcome.state === "decided" && chosen !== undefined && !on) || undefined
              }
            >
              {answering ? (
                <button
                  type="button"
                  className="decisions-choice"
                  aria-pressed={on}
                  data-decisions-pick={index}
                  onClick={() => onPick(index)}
                >
                  {body}
                </button>
              ) : (
                <div className="decisions-choice">{body}</div>
              )}
            </li>
          )
        })}
      </ul>
      {recommended && ask.reason ? (
        <section className="decisions-detail-block">
          <h4>Why {recommended.label}</h4>
          <p className="decisions-dialog-why">{ask.reason}</p>
        </section>
      ) : null}
    </article>
  )
}
