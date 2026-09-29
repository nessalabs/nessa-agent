import { useLayoutEffect, useRef, useState, type CSSProperties } from "react"
import { ModelPicker, type ModelPickerGroup } from "@nessa-ui/react/model-picker"
import {
  ComposerAccessMode,
  type ComposerAccessModeValue,
} from "@nessa-ui/react/composer-access-mode"
import { ModelThinkingControl } from "@nessa-ui/react/model-capability-controls"
import { AgentMark } from "../../onboarding/ui/agent-mark"
import {
  agentForProvider,
  composerModels,
  composerProviders,
  contextLabel,
  defaultComposerModel,
  defaultThinkingLevel,
  fastModeFor,
  shortModelName,
  thinkingLevelsFor,
  type ComposerModel,
} from "../model/composer-options"
import { nextPageMode } from "../model/page-mode"
import { DesktopIcon } from "./icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuItem,
  MenuLabel,
} from "./menu"
import { tooltip } from "./tooltip"

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

function findComposerModel(
  providerId: string,
  modelId: string,
): ComposerModel | undefined {
  return composerModels.find(
    (model) => model.provider === providerId && model.modelId === modelId,
  )
}

/** How many lines the draft fills, from the text area's height less its padding. */
function draftLines(textarea: HTMLTextAreaElement): number {
  const style = getComputedStyle(textarea)
  const lineHeight = Number.parseFloat(style.lineHeight)
  const padding =
    Number.parseFloat(style.paddingTop) + Number.parseFloat(style.paddingBottom)
  return lineHeight > 0 ? Math.round((textarea.scrollHeight - padding) / lineHeight) : 1
}

/**
 * The home composer: a message, and beneath it the three choices a turn is
 * sent with — model, thinking, and what the agent may do without asking.
 * Nothing is sent yet: the shell has no conversation wiring, and says so.
 */
export function Composer({
  page,
  onPageChange,
  initialModel,
  onModelChange,
  onSend,
  text,
  onTextChange,
  placeholder = "What would you like to work on?",
}: {
  /** Whether the composer shows as a full writing page rather than a card. */
  page: boolean
  onPageChange: (page: boolean) => void
  /** The catalog model to start on, as `provider` and `modelId`; the default otherwise. */
  initialModel?: { provider: string; modelId: string }
  /** Told when the person picks another model. */
  onModelChange?: (model: { provider: string; modelId: string }) => void
  /** Sends a turn; without it nothing can be sent and the send button says so. */
  onSend?: (text: string) => void
  /**
   * What is typed and not sent, held by whoever owns it — the workspace keeps
   * a session's in its state, so it outlives a change of layout — and told of
   * every change. Sending empties it there, not here: the workspace's
   * `sendMessage` lets it go once the message is on its way, and keeps it
   * when there was nothing to send.
   */
  text: string
  onTextChange: (text: string) => void
  placeholder?: string
}) {
  const draft = text
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Measured after each edit, before paint, so the layout changes with the
  // keystroke that crossed a threshold rather than a frame after it.
  useLayoutEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return
    const next = nextPageMode(page, draftLines(textarea))
    if (next !== page) onPageChange(next)
  }, [draft, page, onPageChange])
  const send = () => {
    const message = draft.trim()
    if (!onSend || message === "") return
    onSend(message)
  }
  const [model, setModel] = useState(
    () =>
      composerModels.find(
        (candidate) =>
          candidate.provider === initialModel?.provider &&
          candidate.modelId === initialModel.modelId,
      ) ?? defaultComposerModel(composerModels),
  )
  const [thinking, setThinking] = useState(defaultThinkingLevel)
  const [access, setAccess] = useState<ComposerAccessModeValue>("ask-approval")

  // The model's levels, in the control's terms: the utmost gets its Ultra treatment.
  const levels = thinkingLevelsFor(model).map(({ utmost, ...level }) =>
    utmost ? { ...level, accent: "ultra" as const } : level,
  )
  const [fast, setFast] = useState(false)
  // Fast is remembered while switching models, but only in effect on one that offers it.
  const fastOn = fast && fastModeFor(model)

  return (
    <form
      className="desktop-composer"
      onSubmit={(event) => {
        event.preventDefault()
        send()
      }}
    >
      <div
        className="desktop-composer-body"
        // The page's empty space below the text still places the caret.
        onMouseDown={(event) => {
          if (event.target !== event.currentTarget) return
          event.preventDefault()
          textareaRef.current?.focus()
        }}
      >
        <textarea
          ref={textareaRef}
          aria-label="Message"
          placeholder={placeholder}
          rows={2}
          value={draft}
          onChange={(event) => onTextChange(event.target.value)}
          onKeyDown={(event) => {
            // Return sends; Shift-Return, and Return while composing an IME word, do not.
            if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing)
              return
            event.preventDefault()
            // One press, one act: a held key's repeats do nothing
            // (`takesAnswerKey` in `workspace/model/overview/walk.ts` owns the rule).
            if (!event.repeat) send()
          }}
        />
      </div>
      <div className="desktop-composer-row">
        <div className="desktop-composer-controls">
          <ProjectMenu />
          <span
            className="desktop-tip-anchor"
            // In a narrow composer the chip shows this, in place of the whole name.
            style={
              {
                "--desktop-model-short": JSON.stringify(
                  model ? shortModelName(model) : "",
                ),
              } as CSSProperties
            }
            {...tooltip("Model", { side: "above" })}
          >
            <ModelPicker
              groups={pickerGroups}
              value={model && { providerId: model.provider, modelId: model.modelId }}
              onValueChange={({ providerId, modelId }) => {
                const next = findComposerModel(providerId, modelId)
                if (!next) return
                setModel(next)
                onModelChange?.({ provider: next.provider, modelId: next.modelId })
              }}
              side="top"
              align="start"
              sideOffset={10}
              placeholder="Choose model"
              className="desktop-chip desktop-chip-model"
              contentClassName="desktop-popover desktop-model-picker"
            />
          </span>
        </div>
        <div className="desktop-composer-controls">
          <ComposerAccessMode
            value={access}
            onValueChange={setAccess}
            className="desktop-chip"
            contentClassName="desktop-popover"
            // The window's own tooltip names it; no second, system one.
            title=""
            {...tooltip("Access", { side: "above" })}
          />
          <span
            className="desktop-tip-anchor"
            {...tooltip("Thinking", { side: "above" })}
          >
            <ModelThinkingControl
              levels={levels}
              value={thinking}
              onValueChange={setThinking}
              className="desktop-chip"
              contentClassName="desktop-popover"
              sliderLabel="Thinking"
              align="end"
              fastMode={
                fastModeFor(model)
                  ? { pressed: fast, onPressedChange: setFast }
                  : undefined
              }
              icon={
                <>
                  <DesktopIcon name="thinking" />
                  {fastOn ? (
                    <DesktopIcon name="fast" className="desktop-fast-mark" />
                  ) : null}
                </>
              }
            />
          </span>
          <button
            type="submit"
            className="desktop-send"
            aria-label="Send"
            {...tooltip(onSend ? "Send" : "Chat isn’t connected yet", {
              shortcut: onSend ? "↩" : undefined,
              side: "above",
            })}
            disabled={!onSend || draft.trim() === ""}
          >
            <DesktopIcon name="send" />
          </button>
        </div>
      </div>
    </form>
  )
}

/**
 * Where the conversation will work. There is no native folder picker for this
 * window yet, so opening a folder is shown and disabled rather than faked.
 */
function ProjectMenu() {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="desktop-chip desktop-chip-project"
          aria-label="Project: none chosen"
          {...tooltip("Project", { side: "above" })}
        >
          <DesktopIcon name="folder" />
          <span>Project</span>
          <DesktopIcon name="chevronDown" className="desktop-chip-chevron" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" sideOffset={10}>
        <MenuLabel>Project</MenuLabel>
        <MenuItem disabled {...tooltip("Needs the native folder picker")}>
          <DesktopIcon name="folderAdd" />
          Open folder…
        </MenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
