import { ModelPicker, type ModelPickerGroup } from "@nessa-ui/react/model-picker"
import { AgentMark } from "../../onboarding/ui/agent-mark"
import {
  agentForProvider,
  composerModels,
  composerProviders,
  contextLabel,
  type ComposerModel,
} from "../model/composer-options"
import type { ModelRef } from "../workspace/model/workspace-index"

/**
 * A provider's mark on a small glass tile, like an app icon: the agent's own
 * mark from onboarding where nessa runs one for that provider, and the
 * capitals of its name otherwise.
 */
function ProviderMark({ id, label }: { id: string; label: string }) {
  const agent = agentForProvider(id)
  return (
    <span className="desktop-provider-mark">
      {agent ? (
        <AgentMark id={agent} name={label} />
      ) : (
        label.replace(/[^A-Z]/g, "").slice(0, 2) || label.charAt(0).toUpperCase()
      )}
    </span>
  )
}

/** The SDK catalog in the picker's shape: each model with its context window. */
const pickerGroups: ModelPickerGroup[] = composerProviders.map((provider) => ({
  id: provider.id,
  label: provider.label,
  icon: <ProviderMark id={provider.id} label={provider.label} />,
  models: provider.models.map((model) => ({
    id: model.modelId,
    label: model.displayName,
    // Set per model, not only per provider, so the picker's trigger shows it too.
    icon: <ProviderMark id={provider.id} label={provider.label} />,
    description: contextLabel(model.maxContextWindowTokens),
  })),
}))

/** The catalog's entry for a model, if it lists it. */
export function findComposerModel(
  providerId: string,
  modelId: string,
): ComposerModel | undefined {
  return composerModels.find(
    (model) => model.provider === providerId && model.modelId === modelId,
  )
}

/**
 * The one model picker over the SDK catalog: the composer's chip, and the
 * model a new channel starts on. A pick the catalog does not list is not
 * passed on.
 */
export function CatalogueModelPicker({
  value,
  onChange,
  disabled,
  side,
  className,
}: {
  value: ModelRef | null | undefined
  onChange?: (model: ModelRef) => void
  disabled?: boolean
  side: "top" | "bottom"
  className: string
}) {
  return (
    <ModelPicker
      disabled={disabled || !onChange}
      groups={pickerGroups}
      value={value ? { providerId: value.provider, modelId: value.modelId } : undefined}
      onValueChange={({ providerId, modelId }) => {
        const next = findComposerModel(providerId, modelId)
        if (!next) return
        onChange?.({ provider: next.provider, modelId: next.modelId })
      }}
      side={side}
      align="start"
      sideOffset={10}
      placeholder="Choose model"
      className={className}
      contentClassName="desktop-popover desktop-model-picker"
    />
  )
}
