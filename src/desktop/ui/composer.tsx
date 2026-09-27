import { useLayoutEffect, useRef, useState } from "react"
import { ArrowUp, Brain, ChevronDown, Folder, FolderPlus, Zap } from "lucide-react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@nessa-ui/react/dropdown-menu"
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
  thinkingLevelsFor,
  type ComposerModel,
} from "../model/composer-options"
import { nextPageMode } from "../model/page-mode"

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
}: {
  /** Whether the composer shows as a full writing page rather than a card. */
  page: boolean
  onPageChange: (page: boolean) => void
}) {
  const [draft, setDraft] = useState("")
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Measured after each edit, before paint, so the layout changes with the
  // keystroke that crossed a threshold rather than a frame after it.
  useLayoutEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return
    const next = nextPageMode(page, draftLines(textarea))
    if (next !== page) onPageChange(next)
  }, [draft, page, onPageChange])
  const [model, setModel] = useState(() => defaultComposerModel(composerModels))
  const [thinking, setThinking] = useState(defaultThinkingLevel)
  const [access, setAccess] = useState<ComposerAccessModeValue>("ask-approval")

  const levels = thinkingLevelsFor(model)
  const [fast, setFast] = useState(false)
  // Fast is remembered while switching models, but only in effect on one that offers it.
  const fastOn = fast && fastModeFor(model)

  return (
    <form className="desktop-composer" onSubmit={(event) => event.preventDefault()}>
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
          placeholder="What would you like to work on?"
          rows={2}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
      </div>
      <div className="desktop-composer-row">
        <div className="desktop-composer-controls">
          <ProjectMenu />
          <ModelPicker
            groups={pickerGroups}
            value={model && { providerId: model.provider, modelId: model.modelId }}
            onValueChange={({ providerId, modelId }) => {
              const next = findComposerModel(providerId, modelId)
              if (next) setModel(next)
            }}
            side="top"
            align="start"
            sideOffset={10}
            placeholder="Choose model"
            className="desktop-chip desktop-chip-model"
            contentClassName="desktop-popover desktop-model-picker"
          />
        </div>
        <div className="desktop-composer-controls">
          <ComposerAccessMode
            value={access}
            onValueChange={setAccess}
            className="desktop-chip"
            contentClassName="desktop-popover"
          />
          <ModelThinkingControl
            levels={levels}
            value={thinking}
            onValueChange={setThinking}
            className="desktop-chip"
            contentClassName="desktop-popover"
            sliderLabel="Thinking"
            align="end"
            fastMode={
              fastModeFor(model) ? { pressed: fast, onPressedChange: setFast } : undefined
            }
            icon={
              <>
                <Brain aria-hidden="true" />
                {fastOn ? <Zap aria-hidden="true" className="desktop-fast-mark" /> : null}
              </>
            }
          />
          <button
            type="submit"
            className="desktop-send"
            aria-label="Send"
            title="Chat isn’t connected yet"
            disabled
          >
            <ArrowUp aria-hidden="true" />
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
        >
          <Folder aria-hidden="true" />
          <span>Project</span>
          <ChevronDown aria-hidden="true" className="desktop-chip-chevron" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        align="start"
        sideOffset={10}
        className="desktop-popover desktop-project-menu"
      >
        <DropdownMenuLabel className="desktop-menu-label">Project</DropdownMenuLabel>
        <DropdownMenuItem disabled title="Needs the native folder picker">
          <FolderPlus aria-hidden="true" />
          Open folder…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
