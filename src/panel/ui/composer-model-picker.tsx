import { ModelPicker, type ModelPickerGroup } from "@nessa-ui/react/model-picker"
import type { ModelCatalog, ModelChoice } from "../../conversation"
import { AGENT_CHOICES } from "../../onboarding/model/onboarding"
import { AgentMark } from "../../onboarding/ui/agent-mark"

/** A context window as a short label: "1M", "1.05M", "512K". */
function contextLabel(tokens: number) {
  return tokens >= 1_000_000
    ? `${Number((tokens / 1_000_000).toFixed(2))}M`
    : `${Math.round(tokens / 1_000)}K`
}

/**
 * One provider tab per agent the gateway runs, each listing that agent's
 * models. An agent setup does not list keeps its id as its name and a monogram
 * as its mark, rather than being dropped: the gateway said it runs it.
 */
function groups(catalog: ModelCatalog): ModelPickerGroup[] {
  return catalog.agents.map((entry) => {
    const known = AGENT_CHOICES.find((choice) => choice.id === entry.agent)
    const name = known?.name ?? entry.agent
    const mark = known ? (
      <AgentMark id={known.id} name={name} />
    ) : (
      <span aria-hidden="true" className="nessa-text-2 font-medium">
        {name.slice(0, 1).toUpperCase()}
      </span>
    )
    return {
      id: entry.agent,
      label: name,
      icon: mark,
      models: entry.models.map((model) => ({
        id: model.modelId,
        label: model.displayName,
        description: `${contextLabel(model.maxContextWindowTokens)} context`,
        icon: mark,
      })),
    }
  })
}

/**
 * The composer's model: the agent's mark beside voice, opening one picker with
 * a tab per agent. The trigger shows only the mark, so its name is the
 * accessible label and a tooltip. The tooltip is drawn here, not left to
 * `title`: a native tooltip waits about a second before it appears, which is
 * too long for the one place the model is named.
 *
 * Choosing for a conversation that already exists opens a new tab with the
 * choice; the caller's `onChoose` owns that rule.
 */
export function ComposerModelPicker({
  catalog,
  value,
  onChoose,
}: {
  catalog: ModelCatalog
  value: ModelChoice
  onChoose: (choice: ModelChoice) => void
}) {
  const choices = groups(catalog)
  const current = choices
    .find((group) => group.id === value.agent)
    ?.models.find((model) => model.id === value.model)
  const name = current ? current.label : "Choose model"
  return (
    <span className="nessa-model-hint relative flex shrink-0">
      <ModelPicker
        groups={choices}
        value={{ providerId: value.agent, modelId: value.model }}
        onValueChange={(next) =>
          onChoose({ agent: next.providerId, model: next.modelId })
        }
        side="top"
        align="end"
        triggerLabel={current ? `Model: ${current.label}` : name}
        // Only the mark shows: the label is kept for assistive technology and the
        // chevron is dropped, so the control is the same size as its neighbours.
        className="nessa-composer-control nessa-composer-model px-2.5 [&>span:nth-of-type(2)]:sr-only [&>svg:last-child]:hidden"
        contentClassName="nessa-composer-model-content"
      />
      {/* Hidden from readers: the trigger's label already says it. */}
      <span aria-hidden="true" data-slot="model-hint" className="nessa-model-hint-label">
        {current?.icon ? (
          <span className="flex size-3.5 items-center justify-center [&_svg]:size-3.5">
            {current.icon}
          </span>
        ) : null}
        {name}
      </span>
    </span>
  )
}
