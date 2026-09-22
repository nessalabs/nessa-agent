import { useState } from "react"
import type { Conversation } from "../model"
import { controlConversation } from "../adapters/store/slice"
import { useConversationDispatch } from "../adapters/store/hooks"

import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireDescription,
  QuestionnaireHeader,
  QuestionnaireInput,
  QuestionnaireItem,
  QuestionnaireSubmit,
  QuestionnaireTitle,
} from "@nessa-ui/react/questionnaire"
import { Button } from "@nessa-ui/react/button"

type Asked = NonNullable<Conversation["remote"]>["questions"][number]

/**
 * What the agent is waiting to hear, asked where the conversation is.
 *
 * A question is not a review: nothing is authorised by answering, and the agent
 * simply cannot continue that path until it hears back. So it sits in the
 * transcript on the agent's side, the way its messages do, rather than arriving
 * as a floating approval a person has to clear.
 *
 * Every question may be skipped — that is the agent's own offer, not a
 * shortcut this adds — and skipping everything is what declining sends.
 */
export function ConversationQuestions({
  conversation,
  gatewayAvailable,
}: {
  conversation: Conversation
  gatewayAvailable: boolean
}) {
  const questions = conversation.remote?.questions ?? []
  if (!questions.length) return null
  return (
    <div className="flex flex-col gap-3">
      {questions.map((ask) => (
        <ConversationQuestion
          key={JSON.stringify([ask.executionId, ask.questionId])}
          ask={ask}
          conversation={conversation}
          gatewayAvailable={gatewayAvailable}
        />
      ))}
    </div>
  )
}

function ConversationQuestion({
  ask,
  conversation,
  gatewayAvailable,
}: {
  ask: Asked
  conversation: Conversation
  gatewayAvailable: boolean
}) {
  const dispatch = useConversationDispatch()
  // Selections live here until they are sent: an answer is one act, so the
  // agent hears every question's answer at once rather than one at a time.
  const [chosen, setChosen] = useState<Record<string, string[]>>({})
  const [ownWords, setOwnWords] = useState<Record<string, string>>({})
  const busy = !gatewayAvailable || conversation.controlPending
  const answered = ask.questions.some(
    (question) => chosen[question.key]?.length || ownWords[question.key]?.trim(),
  )

  const send = (
    choices: { key: string; values: string[]; ownWords?: string }[] | null,
  ) => {
    void dispatch(
      controlConversation({
        id: conversation.id,
        control: {
          kind: "answerQuestion",
          executionId: ask.executionId,
          questionId: ask.questionId,
          choices,
        },
      }),
    )
  }

  return (
    <Questionnaire
      aria-label={ask.message}
      aria-busy={conversation.controlPending}
      className="max-w-[85%] self-start"
    >
      <QuestionnaireHeader>
        <QuestionnaireTitle>{ask.message}</QuestionnaireTitle>
      </QuestionnaireHeader>
      {ask.questions.map((question) => (
        <QuestionnaireItem key={question.key} name={question.key}>
          {/* With one question the framing above is the question, so repeating
              it as a title would say the same thing twice. */}
          {ask.questions.length > 1 || question.header ? (
            <QuestionnaireTitle>{question.header ?? question.prompt}</QuestionnaireTitle>
          ) : null}
          {ask.questions.length > 1 && question.header ? (
            <QuestionnaireDescription>{question.prompt}</QuestionnaireDescription>
          ) : null}
          <QuestionnaireChoices
            multiple={question.multiSelect}
            value={chosen[question.key] ?? []}
            onValueChange={(value) =>
              setChosen((current) => ({ ...current, [question.key]: value }))
            }
          >
            {question.options.map((option) => (
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
          {question.freeText ? (
            <QuestionnaireInput
              name={`${question.key}_custom`}
              aria-label={`Your own answer to: ${question.prompt}`}
              placeholder="Or answer in your own words"
              disabled={busy}
              value={ownWords[question.key] ?? ""}
              onChange={(event) =>
                setOwnWords((current) => ({
                  ...current,
                  [question.key]: event.target.value,
                }))
              }
            />
          ) : null}
        </QuestionnaireItem>
      ))}
      <QuestionnaireActions>
        <Button
          variant="ghost"
          disabled={busy}
          onClick={() => {
            send(null)
          }}
        >
          Skip
        </Button>
        <QuestionnaireSubmit
          disabled={busy || !answered}
          onClick={() => {
            send(
              ask.questions
                .map((question) => ({
                  key: question.key,
                  values: chosen[question.key] ?? [],
                  ownWords: ownWords[question.key]?.trim() || undefined,
                }))
                // A question nobody touched is left out rather than sent empty,
                // which is how the agent's own form expresses a skip.
                .filter((choice) => choice.values.length || choice.ownWords),
            )
          }}
        >
          Answer
        </QuestionnaireSubmit>
      </QuestionnaireActions>
    </Questionnaire>
  )
}
