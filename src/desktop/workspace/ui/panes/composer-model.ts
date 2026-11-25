import type { ComposerNote } from "../../../ui/composer"
import type { ModelChoice, ModelControl } from "../../model/model-choice"
import { agentName, agentOf } from "../../model/workspace-index"
import { composerModelById } from "../../../model/composer-options"

/**
 * What the model picker says of the session's model, in its tooltip: why
 * the last pick was not made, when a made one takes effect, or that a change
 * the source left unconfirmed is settled by the next send, which may then run
 * on the model it asked for rather than the one shown. A change on its way
 * says nothing: the picker already shows it, and Send waits for it
 * (`modelChanging`).
 */
export function modelPickerNote(
  choice: ModelChoice | undefined,
  control: ModelControl | undefined,
  busy: boolean,
): ComposerNote | undefined {
  if (
    choice &&
    (choice.state === "refused" || choice.state === "recovery_required") &&
    choice.code === "conversation_configuration_changed"
  )
    return { tone: "warning", text: "This chat can’t be reopened." }
  if (busy) return undefined
  if (choice?.state === "refused")
    return {
      tone: "warning",
      text:
        choice.code === "agent_sign_in_required"
          ? `${agentName(agentOf(choice.model))} isn’t signed in where this chat runs. Sign it in there, then choose it again.`
          : choice.code === "model_change_not_applied"
            ? "Model didn’t change. Choose again to try."
            : "The model change wasn’t made. Choose again to try.",
    }
  if (control?.pending?.status === "recovery_required") {
    const asked = composerModelById(control.pending.requestedModel)
    return {
      tone: "warning",
      text: `The change to ${asked?.displayName ?? control.pending.requestedModel} isn’t confirmed. Sending checks it first.`,
    }
  }
  if (control?.pending) return undefined
  if (choice) return undefined
  if (control?.effect === "nextTurn")
    return { tone: "neutral", text: "Model and thinking. Applies to the next turn." }
  if (control?.effect === "subsequentResponses")
    return {
      tone: "neutral",
      text: "Model and thinking. Applies to subsequent responses.",
    }
  return undefined
}
