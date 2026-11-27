import { Button } from "@nessa-ui/react/button"
import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireDescription,
  QuestionnaireHeader,
  QuestionnaireTitle,
} from "@nessa-ui/react/questionnaire"
import { AccessRequestHost } from "../../../../conversation"
import { chooseAccess, sendMessage } from "../../adapters/store/commands"
import { modelChanging } from "../../adapters/store/model-commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAtWork,
  selectSession,
  selectWaitingDecisions,
} from "../../adapters/store/selectors"
import { agentName, agentOf, type OwnerAsk as Ask } from "../../model/workspace-index"
import { openDecisions } from "../decisions/decisions-view"
import "./owner-ask.css"
import { entry } from "../../application/workspace-state"
import { AutoPickLine } from "../chrome/auto-pick"

/**
 * The decision an agent in this session asked the person, while the session's
 * list row says it is still asked. The chat only says it waits: Decide opens
 * Decisions on it, the one place the person decides, where the brief sits
 * beside the choices. Typing an answer here still answers it; either way the
 * gateway then says nothing is asked and the pointer leaves.
 *
 * An agent's request for more access is the red card, whose round is
 * `AccessRequestHost`'s, the same as the panel's: Approve raises the
 * session's access by the person's own access command, the same as the
 * composer's shield, and answers once the source's conversation says the
 * session runs at it, every fact here as its owner states it.
 */
export function OwnerAsk({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const ask = useWorkspaceSelector((state) => selectSession(state, sessionId)?.ask)
  const working = useWorkspaceSelector((state) => selectAtWork(state, sessionId))
  // Approve waits while the session's access or model is still changing, or
  // a model change is left unconfirmed: the chat runs at nothing settled
  // until then, and settling the change may move it to Ask. (A message does
  // not wait for an unconfirmed change: its send settles it.)
  const runsAt = useWorkspaceSelector((state) => {
    if (
      modelChanging(state.workspace, sessionId) ||
      entry(state.workspace.transcripts, sessionId)?.modelControl?.pending
    )
      return undefined
    const access = entry(state.workspace.transcripts, sessionId)?.access
    return access && access.changing === undefined ? access.mode : undefined
  })
  // Where a change on its way is headed: the source's own, or the person's
  // choice held for a turn's end or asked of the source. A taken choice is
  // not one: the source has it, and its conversation says where it landed.
  const underway = useWorkspaceSelector((state) => {
    const choice = entry(state.workspace.accessChoices, sessionId)
    if (choice?.state === "changing") return choice.next ?? choice.mode
    if (choice?.state === "pending") return choice.mode
    return entry(state.workspace.transcripts, sessionId)?.access?.changing
  })
  const revision = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.revision,
  )
  const answer = (text: string) =>
    dispatch(sendMessage({ sessionId, text, initiator: "person" })).then(
      (outcome) => outcome === "sent",
    )
  // How many other chats wait on the person too, for the pointer's line.
  const others = useWorkspaceSelector(
    (state) =>
      selectWaitingDecisions(state).filter((each) => each.sessionId !== sessionId).length,
  )
  return (
    <>
      <AccessRequestHost
        chat={sessionId}
        ask={ask}
        runsAt={runsAt}
        underway={underway}
        working={working}
        revision={revision}
        raise={(mode) =>
          dispatch(chooseAccess({ sessionId, mode, initiator: "person" })).then(
            // Queued behind a change on its way, it is asked once that
            // settles: for the card, a change on its way like any other
            // (`underway` names where it is headed).
            (outcome) =>
              outcome === "queued"
                ? "taken"
                : outcome === "taken" || outcome === "pending"
                  ? outcome
                  : "refused",
          )
        }
        answer={answer}
      />
      {ask && !ask.access ? (
        <DecisionPointer sessionId={sessionId} ask={ask} others={others} />
      ) : null}
    </>
  )
}

/**
 * A decision waiting in this chat, as the chat says it: who waits, the
 * question, how many choices and which the agent suggests, how many more
 * wait elsewhere, and Decide, which opens Decisions on it.
 */
function DecisionPointer({
  sessionId,
  ask,
  others,
}: {
  sessionId: string
  ask: Ask
  others: number
}) {
  const model = useWorkspaceSelector((state) => selectSession(state, sessionId)?.model)
  const agent = model ? agentName(agentOf(model)) : "The agent"
  const suggestion = ask.choices[ask.recommended]?.label
  const line = [
    `${ask.choices.length} choices`,
    suggestion ? `suggests ${suggestion}` : undefined,
    others ? `${others} more waiting` : undefined,
  ]
    .filter(Boolean)
    .join(" · ")
  return (
    <Questionnaire
      aria-label={ask.question}
      className="decision-pointer"
      data-owner-ask={ask.askId}
    >
      <QuestionnaireHeader>
        <div className="decision-pointer-head">
          <p className="decision-pointer-who">{agent} is waiting on you</p>
          <QuestionnaireTitle>{ask.question}</QuestionnaireTitle>
          <QuestionnaireDescription data-owner-ask-line="">
            {line}
          </QuestionnaireDescription>
        </div>
      </QuestionnaireHeader>
      <AutoPickLine sessionId={sessionId} askId={ask.askId} />
      <QuestionnaireActions>
        <Button data-owner-ask-decide="" onClick={() => openDecisions(sessionId)}>
          Decide
        </Button>
      </QuestionnaireActions>
    </Questionnaire>
  )
}
