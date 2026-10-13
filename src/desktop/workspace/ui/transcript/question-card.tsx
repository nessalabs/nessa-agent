import { useState } from "react"
import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireDescription,
  QuestionnaireHeader,
  QuestionnaireInput,
  QuestionnaireItem,
  QuestionnaireTitle,
} from "@nessa-ui/react/questionnaire"
import { questionAnswerReady, questionChoices } from "../../../../conversation"
import { answerQuestion } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { TranscriptQuestion } from "../../model/transcript"
import { failureCopy } from "../failure-copy"

/**
 * A question the agent is waiting on, drawn where the conversation is.
 *
 * Answering authorises nothing. Skip declines the whole ask. A required
 * field needs one of its options; the shared check (`questionAnswerReady`)
 * is what holds Answer back, on this surface and the panel's.
 *
 * The card's own busy state covers the send. A second card of the same ask
 * — the overview over the pane — is told the first is already on its way.
 */
export function QuestionCard({
  sessionId,
  question,
}: {
  sessionId: string
  question: TranscriptQuestion
}) {
  const dispatch = useWorkspaceDispatch()
  // Maps, not objects: the keys are the agent's own field names.
  const [chosen, setChosen] = useState<ReadonlyMap<string, readonly string[]>>(
    () => new Map(),
  )
  const [ownWords, setOwnWords] = useState<ReadonlyMap<string, string>>(() => new Map())
  const [busy, setBusy] = useState(false)
  const [failure, setFailure] = useState<WorkspaceFailureReason | null>(null)
  const ready = questionAnswerReady(question.questions, chosen, ownWords)

  const send = (
    choices: { key: string; values: string[]; ownWords?: string }[] | null,
  ) => {
    setBusy(true)
    setFailure(null)
    void dispatch(
      answerQuestion({
        sessionId,
        executionId: question.executionId,
        questionId: question.questionId,
        choices,
        initiator: "person",
      }),
    ).then((outcome) => {
      if (outcome.status === "sent" || outcome.status === "answering") return
      setBusy(false)
      if (outcome.status === "refused" || outcome.status === "unknown")
        setFailure(outcome.reason)
    })
  }

  return (
    <Questionnaire
      aria-label={question.message}
      aria-busy={busy}
      className="workspace-question"
      data-question={question.questionId}
    >
      <QuestionnaireHeader>
        <QuestionnaireTitle>{question.message}</QuestionnaireTitle>
      </QuestionnaireHeader>
      {question.questions.map((field) => (
        <QuestionnaireItem key={field.key} name={field.key}>
          {question.questions.length > 1 || field.header ? (
            <QuestionnaireTitle>{field.header ?? field.prompt}</QuestionnaireTitle>
          ) : null}
          {question.questions.length > 1 && field.header ? (
            <QuestionnaireDescription>{field.prompt}</QuestionnaireDescription>
          ) : null}
          {question.questions.length > 1 && field.required ? (
            <QuestionnaireDescription>Choose an answer</QuestionnaireDescription>
          ) : null}
          <QuestionnaireChoices
            aria-required={field.required}
            multiple={field.multiSelect}
            value={[...(chosen.get(field.key) ?? [])]}
            onValueChange={(value) =>
              setChosen((current) => new Map(current).set(field.key, value))
            }
          >
            {field.options.map((option) => (
              <QuestionnaireChoice
                key={option.value}
                value={option.value}
                disabled={busy}
              >
                {option.label}
                {option.description ? (
                  <QuestionnaireDescription>
                    {option.description}
                  </QuestionnaireDescription>
                ) : null}
              </QuestionnaireChoice>
            ))}
          </QuestionnaireChoices>
          {field.freeText ? (
            <QuestionnaireInput
              name={`${field.key}_custom`}
              aria-label={`Your own answer to: ${field.prompt}`}
              placeholder="Or answer in your own words"
              disabled={busy}
              value={ownWords.get(field.key) ?? ""}
              onChange={(event) =>
                setOwnWords((current) =>
                  new Map(current).set(field.key, event.target.value),
                )
              }
            />
          ) : null}
        </QuestionnaireItem>
      ))}
      {failure ? (
        <p className="workspace-question-failure" role="status">
          {failureCopy(failure)}
        </p>
      ) : null}
      <QuestionnaireActions className="workspace-question-actions">
        <button
          type="button"
          className="workspace-button"
          disabled={busy}
          onClick={() => send(null)}
        >
          Skip
        </button>
        <button
          type="button"
          className="workspace-button"
          data-primary
          disabled={busy || !ready}
          onClick={() => send(questionChoices(question.questions, chosen, ownWords))}
        >
          Answer
        </button>
      </QuestionnaireActions>
    </Questionnaire>
  )
}
