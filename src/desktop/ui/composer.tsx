import { useLayoutEffect, useRef, useState } from "react"
import {
  ComposerAccessMode,
  type ComposerAccessModeValue,
} from "@nessa-ui/react/composer-access-mode"
import { ChatAttachmentTile } from "@nessa-ui/react/chat-bubbles"
import { ModelFastMode } from "@nessa-ui/react/model-capability-controls"
import { pastedFiles } from "../adapters/pasted-files"
import type { DraftImage } from "../workspace/model/draft-images"
import { defaultThinkingLevel } from "../model/composer-options"
import { nextPageMode } from "../model/page-mode"
import {
  ComposerModelPicker,
  findComposerModel,
  ModelMark,
} from "./catalogue-model-picker"
import { ComposerSpoken } from "./composer-spoken"
import { DesktopIcon } from "./icons"
import { IconButton } from "./icon-button"
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
 * What a composer control has to say of itself: said by that control, as its
 * tooltip in place of its name, and marked on it when it is a `warning`.
 *
 * The composer itself never says anything. A line of status under the draft
 * pushed the controls down, sat between the person and Send, and outlived
 * what it described; so the composer holds only the draft and its row of
 * controls, and whatever one of them needs to say travels with that control.
 * `composer.test.tsx` and the `no-restricted-*` rules for this file in
 * `eslint.config.js` keep it that way.
 */
export interface ComposerNote {
  readonly tone: "neutral" | "warning"
  readonly text: string
}

/**
 * The shield, held by whoever owns the session's access
 * (`workspace/ui/panes/composer-access.ts`): what it shows, told of a pick,
 * and what it says of the last pick (a refusal, or a model change that moved
 * the chat to Ask).
 */
export interface ComposerAccess {
  readonly value: ComposerAccessModeValue
  readonly onChange: (value: ComposerAccessModeValue) => void
  readonly note?: ComposerNote
}

/**
 * The images and files on a composer's next message, owned outside
 * (`workspace/ui/panes/composer-images.ts`): shown as tiles above the text —
 * an image as its picture, any other file as its name — each with a button
 * that takes it off (`onRemove`); files pasted (`pastedFiles`) or picked with
 * the Attach button are offered to `onPaste`, and `note`, said by the Attach
 * button, is why the last ones offered were not taken.
 */
export interface ComposerImages {
  readonly images: readonly DraftImage[]
  readonly onPaste: (files: File[]) => void
  readonly onRemove: (imageId: string) => void
  readonly note?: ComposerNote
}

/**
 * A voice call from the composer, owned outside (`voice/ui/composer-voice.ts`):
 * the button that starts one. While one talks to this conversation
 * (`active`) the button stays, pressed, and the call's own switches are on
 * the call's tab above the composer (`voice/ui/voice-dock.tsx`): the
 * composer is the same composer during a call.
 */
export interface ComposerVoice {
  readonly active: boolean
  readonly start: () => void
}

/**
 * The composer: a message, and in its row what a person sets for it — attach
 * and the model at its start; at its end what the agent may do without
 * asking and how hard it thinks, then, past a hairline, what acts: a voice
 * call and send.
 */
export function Composer({
  images: attached,
  page,
  onPageChange,
  model: shown,
  onModelChange,
  modelDisabled = false,
  modelNote,
  onSend,
  onStop,
  access,
  voice,
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
  /** What the model picker says of the model: its tooltip, and marked on it as a warning. */
  modelNote?: ComposerNote
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
  const warning = [modelNote, access?.note, attached?.note].find(
    (note) => note?.tone === "warning",
  )

  return (
    <>
      {/* Before the form, so nothing stands under the composer card. */}
      <ComposerSpoken notes={[modelNote, access?.note, attached?.note]} />
      <form
        ref={formRef}
        className="desktop-composer"
        // A turn is running: the composer offers Stop (a quiet pane shows only that).
        data-running={onStop ? "" : undefined}
        // Nothing typed or attached: a quiet pane folds it to a circle (`conversation.css`).
        data-empty={draft === "" && images.length === 0 ? "" : undefined}
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
        <div className="desktop-composer-row">
          <div className="desktop-composer-controls">
            {attached ? (
              <AttachButton onFiles={attached.onPaste} note={attached.note} />
            ) : null}
            <span
              className="desktop-tip-anchor"
              {...said(modelNote)}
              {...tooltip(
                modelNote?.text ??
                  (onModelChange
                    ? "Model and thinking"
                    : "Thinking. Start a new conversation to choose another model"),
                { side: "above" },
              )}
            >
              <ComposerModelPicker
                disabled={modelDisabled}
                value={shown}
                onModelChange={onModelChange}
                thinking={thinking}
                onThinkingChange={setThinking}
                fast={{ pressed: fast, onPressedChange: setFast }}
                className="desktop-chip desktop-chip-model"
              />
            </span>
            <span
              className="desktop-tip-anchor"
              {...tooltip(model?.fastMode ? "Fast mode" : "This model has no Fast mode", {
                side: "above",
              })}
            >
              {/* Fast is remembered while switching models, but only on for one that has it. */}
              <ModelFastMode
                pressed={fast && model?.fastMode === true}
                onPressedChange={setFast}
                disabled={!model?.fastMode}
                className="desktop-chip desktop-composer-fast"
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
              {...said(access?.note)}
              {...tooltip(
                access?.note?.text ?? (access ? "Access" : "Chat isn’t connected yet"),
                {
                  side: "above",
                },
              )}
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
        {/*
         * Shown only where a quiet pane folds an empty composer to a circle
         * (`conversation.css`): focus on it focuses the pane, whose caret lands
         * in the field, and the whole composer grows back out of this corner.
         */}
        <button
          type="button"
          className="desktop-composer-wake"
          aria-label="Write a message"
          // Folded, the controls are hidden: the circle says their warning.
          {...said(warning)}
          {...tooltip(warning?.text ?? "Write a message", { side: "above" })}
          // Focus on the circle is focus into the pane: the pane takes it, and
          // the caret follows into the field (`workspace/adapters/dom/focus.ts`).
          onClick={(event) => event.currentTarget.focus()}
        >
          {shown && model ? (
            <ModelMark provider={shown.provider} />
          ) : (
            <DesktopIcon name="edit" />
          )}
        </button>
      </form>
    </>
  )
}

/**
 * A control's note, for whoever cannot hover it: its words as the control's
 * description, and a warning marked on it (`data-attention`, `styles.css`).
 */
function said(note: ComposerNote | undefined) {
  if (!note) return {}
  return {
    "aria-description": note.text,
    ...(note.tone === "warning" ? { "data-attention": "warning" } : {}),
  }
}

/** The voice button: Talk, or pressed while a call talks to this chat. */
function VoiceControls({ voice }: { voice: ComposerVoice }) {
  return voice.active ? (
    <IconButton
      key="voice"
      icon="voice"
      label="In a call"
      tooltipSide="above"
      className="desktop-voice-start"
      aria-pressed
      aria-disabled
      data-active=""
    />
  ) : (
    <IconButton
      key="voice"
      icon="voice"
      label="Talk"
      tooltipSide="above"
      className="desktop-voice-start"
      onClick={voice.start}
    />
  )
}

/**
 * The composer's Attach button: opens the system's file picker and offers
 * every file picked to `onFiles`, as a paste of them would be. The input is
 * emptied after each pick, so picking the same file again is a pick too.
 */
function AttachButton({
  onFiles,
  note,
}: {
  onFiles: (files: File[]) => void
  note?: ComposerNote
}) {
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
        {...said(note)}
        {...tooltip(note?.text ?? "Attach files", { side: "above" })}
        onClick={() => input.current?.click()}
      >
        <DesktopIcon name="attach" />
      </button>
    </>
  )
}
