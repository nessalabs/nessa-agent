import {
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react"
import {
  ComposerAccessMode,
  type ComposerAccessModeValue,
} from "@nessa-ui/react/composer-access-mode"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"
import { StatusLabel } from "@nessa-ui/react/status-label"
import { useComposerDetailsPreference } from "../adapters/composer-details-preference"
import { pastedFiles } from "../adapters/pasted-files"
import type { DraftImage } from "../workspace/model/draft-images"
import {
  defaultThinkingLevel,
  shortModelName,
  thinkingLevelsFor,
} from "../model/composer-options"
import { otherComposerDetails } from "../model/composer-details"
import { nextPageMode } from "../model/page-mode"
import { CatalogueModelPicker, findComposerModel } from "./catalogue-model-picker"
import { DesktopIcon } from "./icons"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuItem,
  MenuLabel,
} from "./menu"
import { IconButton } from "./icon-button"
import { ThinkingControl } from "./thinking-control"
import { tooltip } from "./tooltip"

const noImages: readonly DraftImage[] = []

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
 * The images and files on a composer's next message, owned outside
 * (`workspace/ui/panes/composer-images.ts`): shown as tiles above the text —
 * an image as its picture, any other file as its name — each with a button
 * that takes it off (`onRemove`); files pasted (`pastedFiles`) or picked with
 * the Attach button are offered to `onPaste`, and `notice` says why the last
 * ones offered were not taken.
 */
export interface ComposerImages {
  readonly images: readonly DraftImage[]
  readonly onPaste: (files: File[]) => void
  readonly onRemove: (imageId: string) => void
  readonly notice?: string
}

/**
 * A voice call from the composer, owned outside (`voice/ui/composer-voice.ts`):
 * the button that starts one, and while one talks to this conversation
 * (`active`) its speaker and microphone switches and End, in place of the
 * voice button.
 */
export interface ComposerVoice {
  readonly active: boolean
  readonly start: () => void
  readonly end: () => void
  readonly microphoneOn: boolean
  readonly speakerOn: boolean
  readonly setMicrophone: (on: boolean) => void
  readonly setSpeaker: (on: boolean) => void
}

/**
 * The composer: a message, and in its row only what a person reaches for on
 * every message — attach, the model, what the agent may do without asking,
 * a voice call, and send. What is set once and then only read stands in a
 * quiet line under the card (`ComposerDetailsLine`): the project, the
 * computer the conversation runs on (`place`), and the thinking level.
 */
export function Composer({
  images: attached,
  page,
  onPageChange,
  model: shown,
  onModelChange,
  modelDisabled = false,
  modelNotice,
  onSend,
  onStop,
  access,
  voice,
  place,
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
  modelDisabled?: boolean
  modelNotice?: string
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
  /** A voice call; without it the composer offers none. */
  voice?: ComposerVoice
  /**
   * Where the conversation's agent runs, in words ("This computer"), from
   * whoever knows it; without it the line under the composer names no place.
   */
  place?: string
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
  /**
   * The images on the next message, held with its text by whoever owns it.
   * Without it a paste of images does nothing here.
   */
  images?: ComposerImages
}) {
  const draft = text
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const formRef = useRef<HTMLFormElement>(null)

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
  const images = attached?.images ?? noImages
  // Images alone are a message too: its text may be empty beside them.
  const empty = draft.trim() === "" && images.length === 0
  const send = () => {
    if (!onSend || empty) return
    onSend(draft.trim())
  }
  const model = shown && findComposerModel(shown.provider, shown.modelId)
  const [thinking, setThinking] = useState(defaultThinkingLevel)

  const [fast, setFast] = useState(false)

  return (
    <>
      <form
        ref={formRef}
        className="desktop-composer"
        // A turn is running: the composer offers Stop (a quiet pane shows only that).
        data-running={onStop ? "" : undefined}
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
          {images.length ? (
            <ul className="desktop-composer-images" aria-label="Attachments to send">
              {images.map((image) => (
                <li
                  key={image.id}
                  className="desktop-composer-image"
                  data-kind={image.kind}
                >
                  {image.kind === "image" ? (
                    <ChatAttachmentTile label={image.name} imageSrc={image.url} />
                  ) : (
                    <ChatAttachmentTile
                      label={image.name}
                      icon={<DesktopIcon name="file" />}
                    />
                  )}
                  <button
                    type="button"
                    className="desktop-composer-image-remove"
                    aria-label={`Remove ${image.name}`}
                    {...tooltip(`Remove ${image.name}`, { side: "above" })}
                    onClick={() => attached?.onRemove(image.id)}
                  >
                    <DesktopIcon name="close" />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          <textarea
            ref={textareaRef}
            aria-label="Message"
            placeholder={placeholder}
            rows={2}
            value={draft}
            onChange={(event) => onTextChange(event.target.value)}
            onPaste={(event) => {
              if (!attached) return
              const files = pastedFiles(event.clipboardData)
              if (files.length === 0) return
              // Files and no text: nothing for the field to insert.
              event.preventDefault()
              attached.onPaste(files)
            }}
            onKeyDown={(event) => {
              // Return sends; Shift-Return, and Return while composing an IME word, do not.
              if (
                event.key !== "Enter" ||
                event.shiftKey ||
                event.nativeEvent.isComposing
              )
                return
              event.preventDefault()
              // One press, one act: a held key's repeats do nothing
              // (`takesAnswerKey` in `workspace/model/overview/walk.ts` owns the rule).
              if (!event.repeat) send()
            }}
          />
        </div>
        {attached?.notice ? (
          <StatusLabel role="status" tone="warning" className="desktop-composer-notice">
            {attached.notice}
          </StatusLabel>
        ) : null}
        {modelNotice ? (
          <StatusLabel role="status" tone="neutral" className="desktop-composer-notice">
            {modelNotice}
          </StatusLabel>
        ) : null}
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
            {attached ? <AttachButton onFiles={attached.onPaste} /> : null}
            <span
              className="desktop-tip-anchor"
              // The chip shows this in place of the whole name ("Opus 5"):
              // the mark beside it already says whose model it is.
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
              <CatalogueModelPicker
                disabled={modelDisabled}
                value={shown}
                onChange={onModelChange}
                side="top"
                className="desktop-chip desktop-chip-model"
              />
            </span>
          </div>
          <div className="desktop-composer-controls">
            <ComposerAccessMode
              value={access?.value ?? "ask-approval"}
              onValueChange={access?.onChange}
              // Its menu ends where the composer ends, not past it.
              alignTo={formRef}
              disabled={!access}
              className="desktop-chip"
              contentClassName="desktop-popover"
              // The window's own tooltip names it; no second, system one.
              title=""
              {...tooltip(access ? "Access" : "Chat isn’t connected yet", {
                side: "above",
              })}
            />
            {/* Settings to its left, what acts to its right. */}
            <span className="desktop-composer-divider" aria-hidden="true" />
            {voice ? <VoiceControls voice={voice} /> : null}
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
                disabled={!onSend || empty}
              >
                <DesktopIcon name="send" />
              </button>
            )}
          </div>
        </div>
      </form>
      <ComposerDetailsLine place={place}>
        <span className="desktop-tip-anchor" {...tooltip("Thinking", { side: "above" })}>
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
      </ComposerDetailsLine>
    </>
  )
}

/**
 * The line under the composer: the project, the computer the conversation
 * runs on, and — always — the thinking level (`children`). Hide tucks the
 * project and the computer away for every composer in the window, and Show
 * brings them back (`model/composer-details.ts`); the button shows under the
 * pointer or with the keyboard on it, so the line reads as words at rest.
 */
function ComposerDetailsLine({
  place,
  children,
}: {
  place: string | undefined
  children: ReactNode
}) {
  const [details, chooseDetails] = useComposerDetailsPreference()
  const hidden = details === "hidden"
  return (
    <div className="desktop-composer-details" data-details={details}>
      {hidden ? null : (
        <>
          <ProjectMenu />
          {place ? (
            <span className="desktop-composer-place" data-composer-place>
              <DesktopIcon name="computer" />
              <span>{place}</span>
            </span>
          ) : null}
        </>
      )}
      {children}
      <button
        type="button"
        className="desktop-composer-details-toggle"
        aria-pressed={hidden}
        aria-label={hidden ? "Show project and computer" : "Hide project and computer"}
        onClick={() => chooseDetails(otherComposerDetails(details))}
      >
        {hidden ? "Show" : "Hide"}
      </button>
    </div>
  )
}

/** The voice button, or a call's speaker, microphone and end. */
function VoiceControls({ voice }: { voice: ComposerVoice }) {
  if (!voice.active)
    return (
      <IconButton
        key="voice"
        icon="voice"
        label="Talk"
        tooltipSide="above"
        className="desktop-voice-start"
        onClick={voice.start}
      />
    )
  return (
    <>
      <IconButton
        key="speaker"
        icon={voice.speakerOn ? "speaker" : "speakerOff"}
        label={voice.speakerOn ? "Mute voice" : "Unmute voice"}
        tooltipSide="above"
        className="desktop-voice-toggle"
        aria-pressed={voice.speakerOn}
        onClick={() => voice.setSpeaker(!voice.speakerOn)}
      />
      <IconButton
        key="microphone"
        icon={voice.microphoneOn ? "microphone" : "microphoneOff"}
        label={voice.microphoneOn ? "Mute microphone" : "Unmute microphone"}
        tooltipSide="above"
        className="desktop-voice-toggle"
        aria-pressed={voice.microphoneOn}
        onClick={() => voice.setMicrophone(!voice.microphoneOn)}
      />
      <button
        key="end"
        type="button"
        className="desktop-voice-end"
        aria-label="End call"
        {...tooltip("End call", { side: "above" })}
        onClick={voice.end}
      >
        <DesktopIcon name="endCall" />
        <span>End</span>
      </button>
    </>
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
          <DesktopIcon name="project" />
          <span>Project</span>
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

/**
 * The composer's Attach button: opens the system's file picker and offers
 * every file picked to `onFiles`, as a paste of them would be. The input is
 * emptied after each pick, so picking the same file again is a pick too.
 */
function AttachButton({ onFiles }: { onFiles: (files: File[]) => void }) {
  const input = useRef<HTMLInputElement>(null)
  return (
    <>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        aria-hidden="true"
        tabIndex={-1}
        className="desktop-composer-attach-input"
        onChange={(event) => {
          const files = Array.from(event.currentTarget.files ?? [])
          event.currentTarget.value = ""
          if (files.length) onFiles(files)
        }}
      />
      <button
        type="button"
        className="desktop-chip desktop-composer-attach"
        aria-label="Attach files"
        {...tooltip("Attach files", { side: "above" })}
        onClick={() => input.current?.click()}
      >
        <DesktopIcon name="attach" />
      </button>
    </>
  )
}
