import { useLayoutEffect, useRef, useState, type CSSProperties } from "react"
import { ModelPicker, type ModelPickerGroup } from "@nessa-ui/react/model-picker"
import {
  ComposerAccessMode,
  type ComposerAccessModeValue,
} from "@nessa-ui/react/composer-access-mode"
import { StatusLabel } from "@nessa-ui/react/status-label"
import { AgentMark } from "../../onboarding/ui/agent-mark"
import {
  agentForProvider,
  composerModels,
  composerProviders,
  contextLabel,
  defaultThinkingLevel,
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
import { ThinkingControl } from "./thinking-control"
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
 * The shield, held by whoever owns the session's access
 * (`workspace/ui/panes/composer-access.ts`): what it shows, told of a pick,
 * and what to say of the last pick — waiting for the agent to stop
 * (`neutral`), or refused (`warning`).
 */
export interface ComposerAccess {
  readonly value: ComposerAccessModeValue
  readonly onChange: (value: ComposerAccessModeValue) => void
  readonly notice?: { readonly tone: "neutral" | "warning"; readonly text: string }
}

/**
 * The home composer: a message, and beneath it the three choices a turn is
 * sent with — model, thinking, and what the agent may do without asking.
 * Nothing is sent yet: the shell has no conversation wiring, and says so.
 */
export function Composer({
  page,
  onPageChange,
  model: shown,
  onModelChange,
  onSend,
  onStop,
  access,
  text,
  onTextChange,
  placeholder = "What would you like to work on?",
}: {
  /** Whether the composer shows as a full writing page rather than a card. */
  page: boolean
  onPageChange: (page: boolean) => void
  /**
   * The model the next turn is sent with, as `provider` and `modelId`, held
   * by whoever sends it — the workspace's `modelForNextTurn` for a session —
   * so what the picker shows is what is sent, whoever changed it. A model
   * not in the catalogue shows as none chosen rather than as another one.
   */
  model: { provider: string; modelId: string } | undefined
  /** Told when the person picks another model; the owner decides what is shown. */
  onModelChange?: (model: { provider: string; modelId: string }) => void
  /** Sends a turn; without it nothing can be sent and the send button says so. */
  onSend?: (text: string) => void
  /**
   * Given while the conversation's agent is at work on a turn: the send
   * button becomes Stop, which calls it. Return still sends a draft, which
   * the source takes after the turn. `stopping` shows Stop asked and not
   * yet answered, so it is not asked twice.
   */
  onStop?: { readonly stop: () => void; readonly stopping?: boolean }
  /**
   * The session's access, owned outside: the shield shows `access.value`
   * and asks `access.onChange`. Without it there is no session to change,
   * and the shield is disabled rather than showing a choice nobody keeps.
   */
  access?: ComposerAccess
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
    // An empty draft has one line regardless of a wrapping placeholder or
    // the field's minimum height. Asking layout on mount flushes a whole pane
    // for an answer the draft already owns.
    const next = nextPageMode(page, draft === "" ? 1 : draftLines(textarea))
    if (next !== page) onPageChange(next)
  }, [draft, page, onPageChange])
  const send = () => {
    const message = draft.trim()
    if (!onSend || message === "") return
    onSend(message)
  }
  const model = shown && findComposerModel(shown.provider, shown.modelId)
  const [thinking, setThinking] = useState(defaultThinkingLevel)

  const [fast, setFast] = useState(false)

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
      {access?.notice ? (
        <StatusLabel
          tone={access.notice.tone}
          role="status"
          className="desktop-composer-notice"
        >
          {access.notice.text}
        </StatusLabel>
      ) : null}
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
            {...tooltip(
              onModelChange
                ? "Model"
                : "Start a new conversation to choose another model",
              { side: "above" },
            )}
          >
            <ModelPicker
              disabled={!onModelChange}
              groups={pickerGroups}
              value={shown && { providerId: shown.provider, modelId: shown.modelId }}
              onValueChange={({ providerId, modelId }) => {
                const next = findComposerModel(providerId, modelId)
                if (!next) return
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
            value={access?.value ?? "ask-approval"}
            onValueChange={access?.onChange}
            disabled={!access}
            className="desktop-chip"
            contentClassName="desktop-popover"
            // The window's own tooltip names it; no second, system one.
            title=""
            {...tooltip(access ? "Access" : "Chat isn’t connected yet", {
              side: "above",
            })}
          />
          <span
            className="desktop-tip-anchor"
            {...tooltip("Thinking", { side: "above" })}
          >
            <ThinkingControl
              levels={thinkingLevelsFor(model)}
              value={thinking}
              onValueChange={setThinking}
              // Fast is remembered while switching models, but only offered on one that has it.
              fastMode={
                model && model.fastMode
                  ? { pressed: fast, onPressedChange: setFast }
                  : undefined
              }
            />
          </span>
          {onStop ? (
            <button
              key="stop"
              type="button"
              className="desktop-send"
              aria-label="Stop"
              {...tooltip("Stop", { side: "above" })}
              disabled={onStop.stopping === true}
              onClick={onStop.stop}
            >
              <DesktopIcon name="stop" />
            </button>
          ) : (
            <button
              key="send"
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
          )}
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
