import {
  ModelLevelPicker,
  type ModelLevelPickerGroup,
} from "@nessa-ui/react/model-level-picker"
import { AgentMark, agentMarkDrawing } from "../../onboarding/ui/agent-mark"
import { useStarredModels } from "../adapters/starred-models-preference"
import {
  agentForProvider,
  composerModels,
  composerProviders,
  offeredLevelIndex,
  shortModelName,
  thinkingLevelsFor,
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

/**
 * The mark of the provider a model is from, on its tile — what the composer
 * shows when it is folded to a circle (`composer.tsx`). A provider the
 * catalogue does not list shows nothing.
 */
export function ModelMark({ provider }: { provider: string }) {
  const known = composerProviders.find((each) => each.id === provider)
  return known ? <ProviderMark id={known.id} label={known.label} /> : null
}

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
 * A provider's mark drawn in `currentColor`, so the picker can colour it: in
 * the provider's own colour on a plain tile, and against it on the chosen
 * model's filled one. A provider with no shipped mark gets its capitals.
 */
function ProviderGlyph({ id, label }: { id: string; label: string }) {
  const agent = agentForProvider(id)
  const drawing = agent ? agentMarkDrawing(agent) : undefined
  if (!drawing)
    return (
      <>{label.replace(/[^A-Z]/g, "").slice(0, 2) || label.charAt(0).toUpperCase()}</>
    )
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fillRule="evenodd">
      <path d={drawing.path} fill="currentColor" />
    </svg>
  )
}

/**
 * The SDK catalogue in the model-and-thinking picker's shape: each provider
 * with its mark and, where its mark has a colour of its own, that colour;
 * each model with its short name and the thinking levels its entry lists.
 * A "none" level lights no bar: chosen, the model answers without thinking.
 */
const levelPickerGroups: ModelLevelPickerGroup[] = composerProviders.map((provider) => {
  const agent = agentForProvider(provider.id)
  const fill = agent ? agentMarkDrawing(agent)?.fill : undefined
  const branded = fill !== undefined && fill !== "currentColor"
  return {
    id: provider.id,
    label: provider.label,
    icon: <ProviderGlyph id={provider.id} label={provider.label} />,
    ...(branded ? { accent: fill, accentForeground: "#fff" } : {}),
    models: provider.models.map((model) => ({
      id: model.modelId,
      label: model.displayName,
      shortLabel: shortModelName(model),
      levels: thinkingLevelsFor(model).map((level) => ({
        value: level.value,
        label: level.label,
        ...(level.value === "none" ? { unlit: true } : {}),
      })),
    })),
  }
})

/**
 * The composer's model and thinking, chosen in one place (`ModelLevelPicker`):
 * a chip naming the model and its level ("Opus 5 · Low") that opens the
 * picker. A model chosen by name keeps the thinking already chosen, shown as
 * near as that model allows (`offeredLevelIndex`); a bar chooses both. A
 * model the catalogue does not list is not passed on, and without
 * `onModelChange` the model stays as it is while thinking can still change.
 * A sideways swipe over a model's row steps its level, as ← → do. Stars
 * are the window's (`starred-models-preference.ts`).
 */
export function ComposerModelPicker({
  value,
  onModelChange,
  thinking,
  onThinkingChange,
  fast,
  disabled,
  className,
}: {
  value: ModelRef | null | undefined
  onModelChange?: (model: ModelRef) => void
  thinking: string
  onThinkingChange: (level: string) => void
  fast: { pressed: boolean; onPressedChange: (pressed: boolean) => void }
  disabled?: boolean
  className: string
}) {
  const [starred, setStarred] = useStarredModels()
  const current = value ? findComposerModel(value.provider, value.modelId) : undefined
  // Where the model cannot change, the others are listed but cannot be chosen;
  // the model in use still takes another level.
  const groups = onModelChange
    ? levelPickerGroups
    : levelPickerGroups.map((group) => ({
        ...group,
        models: group.models.map((model) =>
          group.id === value?.provider && model.id === value.modelId
            ? model
            : { ...model, disabled: true },
        ),
      }))
  return (
    <ModelLevelPicker
      disabled={disabled}
      // A sideways trackpad swipe over a model's row moves its thinking level.
      swipeLevels
      groups={groups}
      value={value ? { providerId: value.provider, modelId: value.modelId } : undefined}
      levelFor={(model, group) => {
        const levels = thinkingLevelsFor(findComposerModel(group.id, model.id))
        return levels.length
          ? levels[offeredLevelIndex(levels, thinking)]?.value
          : undefined
      }}
      onValueChange={({ providerId, modelId, level }) => {
        const next = findComposerModel(providerId, modelId)
        if (!next) return
        // Every pick of a model is passed on, the one shown too: while another
        // client's change is still arriving, choosing the shown model is what
        // keeps it, and the application decides whether anything must change.
        if (onModelChange) {
          onModelChange({ provider: next.provider, modelId: next.modelId })
        } else if (next.provider !== value?.provider || next.modelId !== value?.modelId) {
          return
        }
        if (level !== undefined) onThinkingChange(level)
      }}
      starred={starred.map(({ provider, modelId }) => ({
        providerId: provider,
        modelId,
      }))}
      onStarredChange={(next) =>
        setStarred(
          next.map(({ providerId, modelId }) => ({ provider: providerId, modelId })),
        )
      }
      fast={{
        available: current?.fastMode === true,
        pressed: fast.pressed,
        onPressedChange: fast.onPressedChange,
      }}
      placeholder="Choose model"
      className={className}
    />
  )
}

/** The catalogue without thinking: a new channel chooses only its model. */
const modelOnlyGroups: ModelLevelPickerGroup[] = levelPickerGroups.map((group) => ({
  ...group,
  models: group.models.map(({ levels: _levels, ...model }) => model),
}))

/**
 * The model a new channel starts on, chosen in the same picker as the
 * composer's, with the same stars but no thinking or Fast. A pick the
 * catalogue does not list is not passed on.
 */
export function CatalogueModelPicker({
  value,
  onChange,
  disabled,
  className,
}: {
  value: ModelRef | null | undefined
  onChange?: (model: ModelRef) => void
  disabled?: boolean
  className: string
}) {
  const [starred, setStarred] = useStarredModels()
  return (
    <ModelLevelPicker
      disabled={disabled || !onChange}
      groups={modelOnlyGroups}
      value={value ? { providerId: value.provider, modelId: value.modelId } : undefined}
      onValueChange={({ providerId, modelId }) => {
        const next = findComposerModel(providerId, modelId)
        if (!next) return
        onChange?.({ provider: next.provider, modelId: next.modelId })
      }}
      starred={starred.map(({ provider, modelId }) => ({
        providerId: provider,
        modelId,
      }))}
      onStarredChange={(next) =>
        setStarred(
          next.map(({ providerId, modelId }) => ({ provider: providerId, modelId })),
        )
      }
      placeholder="Choose model"
      className={className}
    />
  )
}
