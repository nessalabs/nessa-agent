import { Badge } from "@nessa-ui/react/badge"
import {
  Questionnaire,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireDescription,
  QuestionnaireHeader,
  QuestionnaireItem,
  QuestionnaireTitle,
} from "@nessa-ui/react/questionnaire"
import type { ReactNode } from "react"
import { type OwnerAskShown, undoDiffers, undoLabels } from "../model/owner-ask"

/**
 * A decision an agent asked the person (`ask_owner`), drawn where the
 * conversation is: the question, each choice with what it does, and the one
 * the agent recommends with why. A summary, when the agent gave one, sits
 * under the question, and each choice says how hard it is to undo when that
 * differs between them.
 *
 * Tapping a choice is the answer: its label goes as the person's own
 * message, the same as typing it. Typing anything else answers too. The card
 * stays until the conversation says nothing is asked, so it shows the choice
 * on its way while the message goes, and offers them all again if the send
 * failed (`failure`). `aside` is a line the surface adds under the choices,
 * such as when the recommended choice will be picked for the person.
 */
export function OwnerAskCard({
  ask,
  sending,
  failure,
  onChoose,
  aside,
}: {
  ask: OwnerAskShown
  /** The label on its way, while it is. */
  sending: string | null
  /** Why the last choice was not sent, in words. */
  failure: string | null
  onChoose: (label: string) => void
  aside?: ReactNode
}) {
  const recommended = ask.choices[ask.recommended]
  const undo = undoDiffers(ask)
  return (
    <Questionnaire
      aria-label={ask.question}
      aria-busy={sending !== null}
      className="conversation-owner-ask"
      data-owner-ask={ask.askId}
    >
      <QuestionnaireHeader>
        <QuestionnaireTitle>{ask.question}</QuestionnaireTitle>
        {ask.summary ? (
          <QuestionnaireDescription data-owner-ask-summary>
            {ask.summary}
          </QuestionnaireDescription>
        ) : null}
        {recommended ? (
          <QuestionnaireDescription data-owner-ask-reason>
            Recommended: {recommended.label}. {ask.reason}
          </QuestionnaireDescription>
        ) : null}
      </QuestionnaireHeader>
      <QuestionnaireItem name={`ask-${ask.askId}`}>
        <QuestionnaireChoices
          value={sending === null ? [] : [sending]}
          onValueChange={(value) => {
            const label = value[0]
            if (label !== undefined && sending === null) onChoose(label)
          }}
        >
          {ask.choices.map((choice, index) => (
            <QuestionnaireChoice
              key={choice.label}
              value={choice.label}
              disabled={sending !== null}
              data-owner-ask-choice={choice.label}
            >
              {choice.label}
              {index === ask.recommended ? (
                <Badge variant="secondary" data-owner-ask-recommended>
                  Recommended
                </Badge>
              ) : null}
              {undo && choice.undo ? (
                <Badge variant="outline" data-owner-ask-undo={choice.undo}>
                  {undoLabels[choice.undo]}
                </Badge>
              ) : null}
              <QuestionnaireDescription>{choice.consequence}</QuestionnaireDescription>
            </QuestionnaireChoice>
          ))}
        </QuestionnaireChoices>
      </QuestionnaireItem>
      {aside}
      {failure ? (
        <QuestionnaireDescription role="status" data-owner-ask-failure>
          {failure}
        </QuestionnaireDescription>
      ) : (
        <QuestionnaireDescription>
          Tap a choice to answer, or write your own reply.
        </QuestionnaireDescription>
      )}
    </Questionnaire>
  )
}
