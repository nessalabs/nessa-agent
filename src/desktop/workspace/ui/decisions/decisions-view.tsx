/**
 * Decisions: the one place the person decides what agents asked them, across
 * every chat. A decision card in a chat only says one waits and opens this.
 *
 * Down the side, the decision in front: who asks and what they are doing,
 * the question, its choices (and "Something else", in the person's own
 * words), Later and Decide. Beside it, the brief the person reads to decide
 * well, in the order that helps (SBAR): what the agent said it is about, the
 * options side by side on the facts the agent gave, then what the agent
 * would do and why, after the options so the suggestion does not frame them.
 *
 * The queue is the gateway's list (`selectWaitingDecisions`): a decision
 * leaves when the chat's row says nothing is asked there any more. An answer
 * is the person's own message in that chat, as typing it would be.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { AutoPickLine } from "../chrome/auto-pick"
import { Button } from "@nessa-ui/react/button"
import { CheckboxList } from "@nessa-ui/react/checkbox-list"
import { Dialog } from "@nessa-ui/react/dialog"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { Meter } from "@nessa-ui/react/meter"
import {
  Questionnaire,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireInput,
  QuestionnaireItem,
  QuestionnaireTitle,
} from "@nessa-ui/react/questionnaire"
import { SectionedListbox } from "@nessa-ui/react/sectioned-listbox"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@nessa-ui/react/table"
import { useNow } from "../../../../clock"
import { undoLabels } from "../../../../conversation"
import { DesktopIcon } from "../../../ui/icons"
import { sendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectWaitingDecisions } from "../../adapters/store/selectors"
import { ago } from "../../model/decisions"
import {
  acceptedByDefault,
  answerText,
  decisionsByAgent,
  optionFacts,
  suggested,
  type OptionFact,
  type WaitingDecision,
} from "../../model/waiting-decisions"
import type { OwnerAsk } from "../../model/workspace-index"
import "./decisions-view.css"

const openEvent = "nessa:open-decisions"

/** Opens Decisions from anywhere in the window, on a chat's decision when one is named. */
export function openDecisions(sessionId?: string) {
  window.dispatchEvent(
    new CustomEvent<string | undefined>(openEvent, { detail: sessionId }),
  )
}

/**
 * Whether Decisions is open, on which chat's decision, and a way to close
 * it. The window holds this, so what lies under it is inert while it is open.
 */
export function useDecisionsOpening(): {
  open: boolean
  on: string | undefined
  close: () => void
} {
  const [opened, setOpened] = useState<{ on: string | undefined } | null>(null)
  useEffect(() => {
    const show = (event: Event) => {
      const asked = event instanceof CustomEvent ? event.detail : undefined
      setOpened({ on: typeof asked === "string" ? asked : undefined })
    }
    window.addEventListener(openEvent, show)
    return () => window.removeEventListener(openEvent, show)
  }, [])
  const close = useCallback(() => setOpened(null), [])
  return { open: opened !== null, on: opened?.on, close }
}

export function DecisionsHost({
  open,
  on,
  onClose,
}: {
  open: boolean
  on: string | undefined
  onClose: () => void
}) {
  return open ? <DecisionsSurface on={on} onClose={onClose} /> : null
}

/** "Something else": the person's own words, not one of the agent's choices. */
const own = "own"

/** What the person has picked on a decision but not sent: kept while they look at others. */
type Draft = { readonly choice: number } | { readonly own: string }

type Mode = "decide" | "queue" | "accept"

function DecisionsSurface({
  on,
  onClose,
}: {
  on: string | undefined
  onClose: () => void
}) {
  const dispatch = useWorkspaceDispatch()
  const now = useNow(60_000)
  const listed = useWorkspaceSelector(selectWaitingDecisions)
  const [current, setCurrent] = useState<string | undefined>(on)
  const [mode, setMode] = useState<Mode>("decide")
  const [drafts, setDrafts] = useState<Readonly<Record<string, Draft>>>({})
  const [sending, setSending] = useState<ReadonlySet<string>>(new Set())
  const [failures, setFailures] = useState<Readonly<Record<string, string>>>({})
  // Asks answered here while it was open, for "N of M decided". One the
  // gateway took is answered: it is not offered again in the moment before
  // the chat's row says nothing is asked there.
  const [decided, setDecided] = useState<ReadonlySet<string>>(new Set())
  const decisions = useMemo(
    () => listed.filter((each) => !decided.has(each.ask.askId)),
    [listed, decided],
  )
  // Where the one in front was, so when it leaves the next takes its place.
  const place = useRef(0)
  const found = decisions.findIndex((each) => each.sessionId === current)
  if (found >= 0) place.current = found
  const decision =
    found >= 0
      ? decisions[found]
      : decisions[Math.min(place.current, decisions.length - 1)]

  const send = useCallback(
    (each: WaitingDecision, text: string) => {
      const askId = each.ask.askId
      setSending((held) => new Set(held).add(askId))
      setFailures(({ [askId]: _, ...rest }) => rest)
      return dispatch(
        sendMessage({ sessionId: each.sessionId, text, initiator: "person" }),
      ).then((outcome) => {
        setSending((held) => {
          const next = new Set(held)
          next.delete(askId)
          return next
        })
        if (outcome === "sent") {
          setDecided((held) => new Set(held).add(askId))
          return true
        }
        setFailures((held) => ({
          ...held,
          [askId]:
            outcome === "not-asked"
              ? "Not sent: this chat no longer asks it."
              : "Not sent. Decide again, or answer in the chat.",
        }))
        return false
      })
    },
    [dispatch],
  )

  const total = decided.size + decisions.length
  const progress = (
    <span className="decisions-view-progress" data-decisions-progress="">
      <span>
        <b>{decided.size}</b> of {total} decided
      </span>
      <Meter
        className="decisions-view-meter"
        label="Decisions answered"
        value={total ? decided.size / total : 0}
        valueText={`${decided.size} of ${total} decided`}
      />
    </span>
  )

  // The decision in front when a send settles: a reply that comes back
  // after the person moved on must not move them again.
  const inFront = useRef<string | undefined>(undefined)
  inFront.current = decision?.ask.askId
  // Focus goes to the next decision's choices when the person moved on, not
  // back to the dialog's top; not on opening, where the dialog takes it.
  const [moved, setMoved] = useState(false)
  const advance = (from: WaitingDecision) => {
    const at = decisions.findIndex((each) => each.sessionId === from.sessionId)
    const next = decisions[at + 1] ?? decisions.find((each) => each !== from)
    setCurrent(next?.sessionId)
    setMoved(true)
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
      size="full"
      title="Decisions"
      description={progress}
      className="decisions-view"
      data-decisions-view=""
      aside={
        decisions.length === 0 ? null : (
          <>
            <div className="decisions-view-bar">
              <Button
                variant="ghost"
                size="sm"
                aria-expanded={mode === "queue"}
                data-decisions-queue-toggle=""
                onClick={() => setMode(mode === "queue" ? "decide" : "queue")}
              >
                {decisions.length} waiting
                <DesktopIcon
                  name="chevronDown"
                  size={12}
                  aria-hidden="true"
                  className="decisions-view-chevron"
                  data-open={mode === "queue" || undefined}
                />
              </Button>
              <Button
                variant={mode === "accept" ? "secondary" : "ghost"}
                size="sm"
                aria-pressed={mode === "accept"}
                data-decisions-accept-all=""
                onClick={() => setMode(mode === "accept" ? "decide" : "accept")}
              >
                Accept all
              </Button>
            </div>
            {mode === "queue" ? (
              <SectionedListbox
                className="decisions-view-queue"
                listLabel="Waiting decisions"
                sections={decisionsByAgent(decisions).map(
                  ({ agent, decisions: held }) => ({
                    id: agent,
                    label: agent,
                    items: held,
                  }),
                )}
                sectionLabelClassName="decisions-view-queue-agent"
                optionClassName="decisions-view-queue-option"
                getItemId={(each) => each.sessionId}
                value={decision?.sessionId}
                onValueChange={(sessionId) => {
                  setCurrent(sessionId)
                  setMode("decide")
                }}
                renderItem={(each) => (
                  <span
                    className="decisions-view-queue-row"
                    data-decisions-row={each.sessionId}
                  >
                    <span className="decisions-view-queue-question">
                      {each.ask.question}
                    </span>
                    <span className="decisions-view-queue-line">
                      {each.title} · {ago(each.askedAtMs, now)}
                    </span>
                  </span>
                )}
              />
            ) : decision ? (
              <DecideColumn
                key={decision.ask.askId}
                decision={decision}
                takeFocus={moved}
                draft={drafts[decision.ask.askId]}
                sending={sending.has(decision.ask.askId)}
                failure={failures[decision.ask.askId]}
                onDraft={(draft) =>
                  setDrafts((held) => ({ ...held, [decision.ask.askId]: draft }))
                }
                onLater={decisions.length > 1 ? () => advance(decision) : undefined}
                onDecide={(text) => {
                  void send(decision, text).then((sent) => {
                    // The answered one leaves the queue, and the one after it
                    // takes its place; only if the person is still on it.
                    if (sent && inFront.current === decision.ask.askId) advance(decision)
                  })
                }}
              />
            ) : null}
          </>
        )
      }
    >
      {decisions.length === 0 ? (
        <EmptyState
          className="decisions-view-empty"
          data-decisions-empty=""
          title={decided.size ? "All decided" : "Nothing waits on you"}
          description={
            decided.size
              ? `You answered ${decided.size} ${decided.size === 1 ? "decision" : "decisions"}. The agents carry on.`
              : "When an agent needs you to decide something, it shows up here."
          }
          action={
            <Button variant="secondary" onClick={onClose}>
              Back to work
            </Button>
          }
        />
      ) : mode === "accept" ? (
        <AcceptAll
          decisions={decisions}
          sending={sending}
          failures={failures}
          onSend={(chosen) => {
            void Promise.all(
              chosen.map((each) => {
                const choice = suggested(each.ask)
                return choice ? send(each, choice.label) : Promise.resolve(false)
              }),
            ).then((sent) => {
              // Back to deciding once every answer went, unless the person
              // already left; any that did not go stay listed with why.
              if (sent.every(Boolean))
                setMode((held) => (held === "accept" ? "decide" : held))
            })
          }}
        />
      ) : decision ? (
        <Brief decision={decision} now={now} />
      ) : null}
    </Dialog>
  )
}

/**
 * The decision in front: who asks and what they are doing, the question,
 * its choices with the suggested one marked, "Something else" for the
 * person's own words, and Later and Decide. Picking is not deciding: nothing
 * goes until Decide.
 */
function DecideColumn({
  decision,
  takeFocus,
  draft,
  sending,
  failure,
  onDraft,
  onLater,
  onDecide,
}: {
  decision: WaitingDecision
  /** Whether the person moved here from another decision, so focus follows them. */
  takeFocus: boolean
  draft: Draft | undefined
  sending: boolean
  failure: string | undefined
  onDraft: (draft: Draft) => void
  onLater: (() => void) | undefined
  onDecide: (text: string) => void
}) {
  const { ask } = decision
  const ownInput = useRef<HTMLInputElement>(null)
  const column = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!takeFocus) return
    const choices = column.current?.querySelectorAll<HTMLInputElement>("input")
    const target = [...(choices ?? [])].find((input) => input.checked) ?? choices?.[0]
    target?.focus({ preventScroll: true })
    // Only as it arrives: a later render keeps focus where the person put it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  const picked =
    draft === undefined ? [] : "own" in draft ? [own] : [String(draft.choice)]
  const text = draft ? answerText(ask, draft) : undefined
  // Choosing "Something else" puts the person straight into writing.
  const choosingOwn = draft !== undefined && "own" in draft
  useEffect(() => {
    if (choosingOwn) ownInput.current?.focus()
  }, [choosingOwn])
  const decide = () => {
    if (text && !sending) onDecide(text)
  }
  return (
    <Questionnaire
      ref={column}
      className="decisions-view-decide"
      aria-busy={sending || undefined}
      data-decisions-decide={ask.askId}
    >
      <p className="decisions-view-who">
        {decision.agent}
        {decision.doing ? ` · ${decision.doing}` : " is waiting on you"}
      </p>
      <QuestionnaireItem name={`decide-${ask.askId}`}>
        <QuestionnaireTitle className="decisions-view-question">
          {ask.question}
        </QuestionnaireTitle>
        <QuestionnaireChoices
          variant="plain"
          value={picked}
          onValueChange={(value) => {
            const chosen = value[0]
            if (chosen === undefined || sending) return
            onDraft(chosen === own ? { own: "" } : { choice: Number(chosen) })
          }}
        >
          {ask.choices.map((choice, index) => (
            <QuestionnaireChoice
              key={choice.label}
              value={String(index)}
              disabled={sending}
              data-decisions-choice={choice.label}
            >
              <span className="decisions-view-choice">
                {choice.label}
                {index === ask.recommended ? (
                  <span className="decisions-view-suggested" data-decisions-suggested="">
                    Suggested
                  </span>
                ) : null}
              </span>
            </QuestionnaireChoice>
          ))}
          <QuestionnaireChoice
            value={own}
            disabled={sending}
            data-decisions-choice-own=""
            className="decisions-view-own"
          >
            <span className="decisions-view-choice">Something else</span>
          </QuestionnaireChoice>
        </QuestionnaireChoices>
        {choosingOwn ? (
          <QuestionnaireInput
            ref={ownInput}
            aria-label="Your answer"
            placeholder={`Tell ${decision.agent} what to do`}
            value={"own" in draft ? draft.own : ""}
            disabled={sending}
            data-decisions-own=""
            onChange={(event) => onDraft({ own: event.target.value })}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault()
                decide()
              }
            }}
          />
        ) : null}
      </QuestionnaireItem>
      {/* Auto-pick keeps counting while the person reads the brief, so its
          countdown and "Wait for me" are here, not only in the chat. */}
      <div className="decisions-view-auto-pick">
        <AutoPickLine sessionId={decision.sessionId} askId={ask.askId} />
      </div>
      {failure ? (
        <p role="status" className="decisions-view-failure" data-decisions-failure="">
          {failure}
        </p>
      ) : null}
      <div className="decisions-view-actions">
        {onLater ? (
          <Button variant="ghost" data-decisions-later="" onClick={onLater}>
            Later
          </Button>
        ) : null}
        <Button
          variant={text ? "default" : "secondary"}
          disabled={!text || sending}
          data-decisions-send=""
          onClick={decide}
        >
          {sending ? "Sending" : "Decide"}
        </Button>
      </div>
    </Questionnaire>
  )
}

const factNames: Record<OptionFact, string> = {
  consequence: "What happens",
  pros: "For",
  cons: "Against",
  effort: "Effort",
  undo: "Undo",
}

const effortWords = { small: "Small", medium: "Medium", large: "Large" } as const

/** Undo in a row already named Undo. */
const undoWords = { easy: "Easy", costly: "Costly", no: "Can't" } as const

/**
 * What the person reads to decide: what the agent said it is about, the
 * options side by side on the facts it gave, then what it would do and why.
 * Each section shows only when the agent gave something for it.
 */
function Brief({ decision, now }: { decision: WaitingDecision; now: number }) {
  const { ask } = decision
  const facts = useMemo(() => optionFacts(ask), [ask])
  const choice = suggested(ask)
  return (
    <article className="decisions-view-brief" data-decisions-brief={ask.askId}>
      <header className="decisions-view-brief-head">
        <h3 className="decisions-view-headline">{decision.title}</h3>
        <p className="decisions-view-meta">
          {decision.agent} asked {ago(decision.askedAtMs, now)}
        </p>
      </header>
      {ask.summary ? (
        <section
          className="decisions-view-section"
          aria-labelledby={`situation-${ask.askId}`}
        >
          <h4 id={`situation-${ask.askId}`}>Situation</h4>
          <p className="decisions-view-prose">{ask.summary}</p>
        </section>
      ) : null}
      <section
        className="decisions-view-section"
        aria-labelledby={`options-${ask.askId}`}
      >
        <h4 id={`options-${ask.askId}`}>Options</h4>
        <Table edgeFade className="decisions-view-options" data-decisions-options="">
          <TableHeader>
            <TableRow>
              <TableHead>
                <span className="sr-only">Fact</span>
              </TableHead>
              {ask.choices.map((each, index) => (
                <TableHead
                  key={each.label}
                  data-suggested={index === ask.recommended || undefined}
                >
                  {each.label}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {facts.map((fact) => (
              <TableRow key={fact} data-fact={fact}>
                <TableHead scope="row" className="decisions-view-fact">
                  {factNames[fact]}
                </TableHead>
                {ask.choices.map((each) => (
                  <TableCell
                    key={each.label}
                    data-undo={fact === "undo" ? each.undo : undefined}
                  >
                    <FactCell choice={each} fact={fact} />
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </section>
      {choice ? (
        <section
          className="decisions-view-section"
          aria-labelledby={`would-${ask.askId}`}
        >
          <h4 id={`would-${ask.askId}`}>What {decision.agent} would do</h4>
          <p className="decisions-view-prose" data-decisions-reason="">
            <b>{choice.label}.</b> {ask.reason}
          </p>
        </section>
      ) : null}
    </article>
  )
}

function FactCell({
  choice,
  fact,
}: {
  choice: OwnerAsk["choices"][number]
  fact: OptionFact
}) {
  switch (fact) {
    case "consequence":
      return <>{choice.consequence}</>
    case "pros":
    case "cons": {
      const lines = choice[fact] ?? []
      return lines.length ? (
        <ul className="decisions-view-lines" data-sign={fact === "pros" ? "+" : "-"}>
          {lines.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      ) : (
        <span className="decisions-view-none">—</span>
      )
    }
    case "effort":
      return choice.effort ? (
        <>{effortWords[choice.effort]}</>
      ) : (
        <span className="decisions-view-none">—</span>
      )
    case "undo":
      return choice.undo ? <>{undoWords[choice.undo]}</> : null
  }
}

/**
 * Accept all: every waiting decision with the choice its agent suggests,
 * ticked when that choice is easy to undo and left for the person to tick
 * otherwise. Send answers each ticked one with its suggestion.
 */
function AcceptAll({
  decisions,
  sending,
  failures,
  onSend,
}: {
  decisions: readonly WaitingDecision[]
  sending: ReadonlySet<string>
  failures: Readonly<Record<string, string>>
  onSend: (chosen: readonly WaitingDecision[]) => void
}) {
  // Ticks are held by ask, not by chat: a chat whose agent replaced its
  // question while this was open starts its new one unticked, so nothing
  // the person has not seen is sent.
  const [ticked, setTicked] = useState<readonly string[]>(() =>
    decisions.filter(acceptedByDefault).map((each) => each.ask.askId),
  )
  const busy = decisions.some((each) => sending.has(each.ask.askId))
  const chosen = decisions.filter((each) => ticked.includes(each.ask.askId))
  return (
    <section
      className="decisions-view-accept"
      data-decisions-accept=""
      aria-labelledby="accept-all"
    >
      <header className="decisions-view-brief-head">
        <h3 id="accept-all" className="decisions-view-headline">
          Accept the suggested answers
        </h3>
        <p className="decisions-view-meta">
          Ones that are easy to undo are ticked. Tick the rest only if you are sure.
        </p>
      </header>
      <CheckboxList
        aria-labelledby="accept-all"
        value={ticked}
        disabled={busy}
        onValueChange={setTicked}
        options={decisions.map((each) => {
          const choice = suggested(each.ask)
          return {
            value: each.ask.askId,
            label: each.ask.question,
            description: (
              <span
                className="decisions-view-accept-line"
                data-decisions-accept-row={each.sessionId}
              >
                <span>{choice ? `Suggests ${choice.label}` : "No suggestion"}</span>
                {choice?.undo && choice.undo !== "easy" ? (
                  <span className="decisions-view-undo" data-undo={choice.undo}>
                    {undoLabels[choice.undo]}
                  </span>
                ) : null}
                <span className="decisions-view-accept-chat">{each.title}</span>
                {failures[each.ask.askId] ? (
                  <span className="decisions-view-failure" data-decisions-failure="">
                    {failures[each.ask.askId]}
                  </span>
                ) : null}
              </span>
            ),
            disabled: !choice,
          }
        })}
      />
      <div className="decisions-view-actions">
        <Button
          disabled={chosen.length === 0 || busy}
          data-decisions-accept-send=""
          onClick={() => onSend(chosen)}
        >
          {chosen.length === 1 ? "Send 1 answer" : `Send ${chosen.length} answers`}
        </Button>
      </div>
    </section>
  )
}
