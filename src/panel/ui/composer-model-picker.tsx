import * as React from "react"
import { ModelPicker, type ModelPickerGroup } from "@nessa-ui/react/model-picker"
import type { AgentsListResult } from "@nessa/client"
import { AGENT_CHOICES } from "../../onboarding/model/onboarding"
import { AgentMark } from "../../onboarding/ui/agent-mark"

type ModelChoice = { agent: string; model: string }

/** A context window as a short label: "1M", "1.05M", "512K". */
function contextLabel(tokens: number) {
  return tokens >= 1_000_000
    ? `${Number((tokens / 1_000_000).toFixed(2))}M`
    : `${Math.round(tokens / 1_000)}K`
}

function agentMark(agent: string) {
  const known = AGENT_CHOICES.find((choice) => choice.id === agent)
  const name = known?.name ?? agent
  const mark = known ? (
    <AgentMark id={known.id} name={name} />
  ) : (
    <span aria-hidden="true" className="nessa-text-2 font-medium">
      {name.slice(0, 1).toUpperCase()}
    </span>
  )
  return { name, mark }
}

/**
 * One provider tab per agent the gateway runs, each listing that agent's
 * models. An agent setup does not list keeps its id as its name and a monogram
 * as its mark, rather than being dropped: the gateway said it runs it.
 *
 * The current model is always listed. One the catalog does not name — a
 * conversation still running a model since retired — joins its agent's tab
 * under its own name, so the trigger shows its mark rather than a blank, and
 * the picker names what the conversation actually runs.
 */
function groups(
  catalog: AgentsListResult,
  value: ModelChoice,
  valueName: string | undefined,
): ModelPickerGroup[] {
  const listed: ModelPickerGroup[] = catalog.agents.map((entry) => {
    const { name, mark } = agentMark(entry.agent)
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
  const group = listed.find((item) => item.id === value.agent)
  if (group?.models.some((model) => model.id === value.model)) return listed
  const { name, mark } = agentMark(value.agent)
  const unlisted = {
    id: value.model,
    label: valueName ?? value.model,
    description: "Not in this gateway's list",
    icon: mark,
  }
  return group
    ? listed.map((item) =>
        item === group ? { ...item, models: [...item.models, unlisted] } : item,
      )
    : [...listed, { id: value.agent, label: name, icon: mark, models: [unlisted] }]
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
  valueName,
  onChoose,
}: {
  catalog: AgentsListResult
  value: ModelChoice
  /** What the gateway calls `value`, for a model the catalog does not list. */
  valueName?: string
  onChoose: (choice: ModelChoice) => void
}) {
  // Built once per catalog and value, not on every streamed chunk that
  // re-renders the composer.
  const { agent, model } = value
  const choices = React.useMemo(
    () => groups(catalog, { agent, model }, valueName),
    [catalog, agent, model, valueName],
  )
  const current = choices
    .find((group) => group.id === value.agent)
    ?.models.find((model) => model.id === value.model)
  const name = current?.label ?? value.model
  // Escape hides the hover label until the pointer or focus leaves, so it can
  // be dismissed without moving away.
  const [dismissed, setDismissed] = React.useState(false)
  return (
    <span
      className="nessa-model-hint relative flex shrink-0"
      data-dismissed={dismissed || undefined}
      onKeyDown={(event) => {
        if (event.key === "Escape") setDismissed(true)
      }}
      onPointerLeave={() => setDismissed(false)}
      onBlur={() => setDismissed(false)}
    >
      <ModelPicker
        groups={choices}
        value={{ providerId: value.agent, modelId: value.model }}
        onValueChange={(next) => {
          // Picking the model already in use is not a choice: on a conversation
          // that exists it would open a new tab for nothing.
          if (next.providerId === value.agent && next.modelId === value.model) return
          onChoose({ agent: next.providerId, model: next.modelId })
        }}
        side="top"
        align="end"
        triggerLabel={`Model: ${name}`}
        // Only the mark shows: the label is kept for assistive technology and the
        // chevron is dropped, so the control is the same size as its neighbours.
        // Both rely on the trigger's child order, which a test pins.
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
