/**
 * Decisions in Chat context: the latest three a chat's agents asked the
 * person, waiting first, and "See all", which opens every decision in the
 * chat in a dialog filtered To decide · All · Decided. A row opens the
 * dialog on that decision. A waiting decision is answered there with the
 * same card the transcript draws (`OwnerAsk`), so the answer is still the
 * person's own message.
 */
import { useState } from "react"
import { Badge } from "@nessa-ui/react/badge"
import { Button } from "@nessa-ui/react/button"
import { Dialog } from "@nessa-ui/react/dialog"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { useNow } from "../../../../clock"
import {
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
import { OwnerAsk } from "../transcript/owner-ask"
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
  const decisions =
    read.state === "read" || read.state === "reading" ? read.decisions : undefined
  if (!decisions) {
    return (
      <p className="chat-context-none">
        {read.state === "failed" ? "Decisions could not be read" : "Reading decisions"}
      </p>
    )
  }
  const ordered = decisionsInOrder(decisions)
  if (ordered.length === 0) return <p className="chat-context-none">Nothing asked yet</p>
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

/**
 * Every decision in the chat, filtered. It opens on the one chosen, on To
 * decide when that one waits and on All otherwise; a row shows its full
 * question, its choices and what became of it.
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
  const [selected, setSelected] = useState(openOn)
  const [filter, setFilter] = useState<DecisionsFilter>(() =>
    filterFor(decisions.find((decision) => decision.ask.askId === openOn)),
  )
  const listed = decisionsFiltered(decisions, filter)
  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
      size="lg"
      title="Decisions"
      description={title}
      data-decisions-dialog=""
    >
      <div className="decisions-dialog">
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
            {listed.map((decision) => {
              const open = decision.ask.askId === selected
              return (
                <li key={decision.ask.askId} data-decision={decision.ask.askId}>
                  <button
                    type="button"
                    className="decisions-dialog-item"
                    aria-expanded={open}
                    data-state={decision.outcome.state}
                    onClick={() => setSelected(open ? null : decision.ask.askId)}
                  >
                    <span className="decisions-dot" data-state={decision.outcome.state} />
                    <span className="decisions-dialog-question">
                      {decision.ask.question}
                    </span>
                    <span className="decisions-dialog-line">
                      {decisionLine(decision, now)}
                    </span>
                  </button>
                  {open ? (
                    <DecisionDetail sessionId={sessionId} decision={decision} />
                  ) : null}
                </li>
              )
            })}
          </ul>
        )}
      </div>
    </Dialog>
  )
}

/**
 * One decision in full. A waiting one that is the chat's ask now is the
 * transcript's own card, to answer from here; a settled one lists its
 * choices with the recommended and the chosen marked, and what was said.
 */
function DecisionDetail({
  sessionId,
  decision,
}: {
  sessionId: string
  decision: Decision
}) {
  const live = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.ask?.askId === decision.ask.askId,
  )
  const outcome = decision.outcome
  if (outcome.state === "waiting" && live) {
    return (
      <div className="decisions-dialog-detail">
        <OwnerAsk sessionId={sessionId} />
      </div>
    )
  }
  const chosen = outcome.state === "decided" ? outcome.choice : undefined
  return (
    <div className="decisions-dialog-detail" data-decision-detail={decision.ask.askId}>
      <p className="decisions-dialog-state" data-state={outcome.state}>
        {stateLabel(outcome.state)}
        {outcome.state === "decided" ? `: ${answerLabel(decision)}` : null}
      </p>
      <ul className="decisions-dialog-choices">
        {decision.ask.choices.map((choice, index) => (
          <li
            key={choice.label}
            data-chosen={index === chosen || undefined}
            data-recommended={index === decision.ask.recommended || undefined}
          >
            <span className="decisions-dialog-choice">
              {choice.label}
              {index === decision.ask.recommended ? (
                <Badge variant="secondary">Recommended</Badge>
              ) : null}
            </span>
            <span className="decisions-dialog-line">{choice.consequence}</span>
          </li>
        ))}
      </ul>
      {decision.ask.reason ? (
        <p className="decisions-dialog-why">Why: {decision.ask.reason}</p>
      ) : null}
      {/* What was said, unless it was only the choice's label. */}
      {outcome.state === "decided" &&
      outcome.text &&
      outcome.text !== answerLabel(decision) ? (
        <blockquote className="decisions-dialog-said">{outcome.text}</blockquote>
      ) : null}
    </div>
  )
}
