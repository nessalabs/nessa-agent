/**
 * The "New channel" flow (design B4, "asked in the transcript"): a layer over
 * the session list and the panes that asks one question at a time, the way
 * an agent would. An answered question folds into a ✓ line; clicking the line
 * asks it again. The composer bar at the foot takes the typed answers (the
 * name, the notes) and holds Create, which is ready as soon as the channel
 * has a name: every other question has an answer already. While it asks for
 * files and folders, any dragged onto the window are taken too: the whole
 * flow is the drop target, lit while a drag is over it.
 *
 * The rules are `usecases/channel-creation.ts`; this draws the store's flow
 * and dispatches its commands. The kit draws the controls (`Button`, `Kbd`,
 * `StatusLabel`, `LoadingText`, the catalogue's `ModelPicker`).
 *
 * Mounted once by the shell; it draws nothing while the flow is closed.
 */
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
} from "react"
import { Button } from "@nessa-ui/react/button"
import { Kbd } from "@nessa-ui/react/kbd"
import { LoadingText } from "@nessa-ui/react/loading-text"
import { StatusLabel } from "@nessa-ui/react/status-label"
import { isMac } from "../../../adapters/platform"
import { inModal } from "../../../adapters/modal"
import { composerProviders } from "../../../model/composer-options"
import {
  CatalogueModelPicker,
  findComposerModel,
} from "../../../ui/catalogue-model-picker"
import { DesktopIcon } from "../../../ui/icons"
import type { ChannelCreation } from "../../application/workspace-state"
import {
  addChannelResources,
  answerChannelStep,
  askChannelStep,
  chooseChannelFolder,
  closeChannelCreation,
  createChannel,
  draftChannel,
  removeChannelResource,
  watchChannelDrops,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAllChannels,
  selectChannelCreation,
  selectChannelMaking,
} from "../../adapters/store/selectors"
import {
  canCreate,
  channelRules,
  channelStepsFor,
  channelsIn,
  failureStep,
  failureText,
  homeShort,
  nameFits,
  notesFit,
  pathName,
  type ChannelDraft,
  type ChannelStep,
} from "../../model/channel-draft"
import type { ModelRef } from "../../model/workspace-index"
import "./new-channel.css"

/** The question each step asks. */
const questions: Record<ChannelStep, string> = {
  name: "What should this channel be called?",
  folder: "Where should its agents work?",
  model: "Which model should new conversations start on?",
  notes: "Anything every agent here should know?",
  resources: "Any files or folders agents here should have on hand?",
}

/** What the composer says while each step is asked; null asks with the composer's field. */
const composerHints: Record<ChannelStep, string> = {
  name: "Name the channel",
  folder: "Pick above, then press ↩",
  model: "Pick above, then press ↩",
  notes: "Notes for agents here, or ↩ to skip",
  resources: "Add or drop files, or create",
}

/** The steps whose answer is typed in the composer's field. */
const typed = (step: ChannelStep | null): step is "name" | "notes" =>
  step === "name" || step === "notes"

/** Create's chord: ⌘↩ on a Mac, Ctrl+↩ elsewhere. */
const createChord = isMac ? "⌘↩" : "Ctrl ↩"
const isCreateChord = (event: { key: string; metaKey: boolean; ctrlKey: boolean }) =>
  event.key === "Enter" && (isMac ? event.metaKey : event.ctrlKey)

/** A model as a line names it: its name, then its provider. */
function modelWords(model: ModelRef): { name: string; provider: string } {
  const listed = findComposerModel(model.provider, model.modelId)
  const provider = composerProviders.find((each) => each.id === model.provider)
  return {
    name: listed?.displayName ?? model.modelId,
    provider: provider?.label ?? model.provider,
  }
}

/** The host for the flow: draws it while it is open. */
export function NewChannelHost() {
  const flow = useWorkspaceSelector(selectChannelCreation)
  return flow ? <NewChannel flow={flow} /> : null
}

function NewChannel({ flow }: { flow: ChannelCreation }) {
  const dispatch = useWorkspaceDispatch()
  const { pickable } = useWorkspaceSelector(selectChannelMaking)
  const layer = useRef<HTMLElement>(null)
  const field = useRef<HTMLTextAreaElement>(null)
  const steps = channelStepsFor(pickable)
  const { draft, asking, creating, failure } = flow
  const landing = flow.landing.length > 0
  const ready = canCreate(draft) && !creating && !landing
  // Watched the whole time the flow is open, so a drop's paths are heard
  // even after the question moves on; only the files question is lit.
  const dropping = useDrops(!creating) && asking === "resources"

  useCovers(layer)
  useFocusReturns()
  // Escape cancels and the chord creates, from anywhere in the window but a
  // menu or a popover over it, which takes its own keys. Create asks nothing
  // of a draft it cannot make (`createChannel`).
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || inModal(event.target)) return
      if (event.key === "Escape") {
        event.preventDefault()
        event.stopPropagation()
        dispatch(closeChannelCreation())
      } else if (isCreateChord(event)) {
        event.preventDefault()
        event.stopPropagation()
        void dispatch(createChannel())
      }
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [dispatch])
  // The keyboard goes where the question is answered: the composer's field
  // for a typed answer, the question's first control otherwise.
  // Asked by a pick, the question's own row takes the keyboard, so Return
  // goes on rather than pressing a pick; with every question answered,
  // Create does.
  useLayoutEffect(() => {
    if (typed(asking)) field.current?.focus()
    else if (asking)
      layer.current
        ?.querySelector<HTMLElement>(".new-channel-asked [data-answers]")
        ?.focus()
    else layer.current?.querySelector<HTMLElement>(".new-channel-create")?.focus()
  }, [asking])

  const failedHere = failure && failureStep(failure) === asking ? failure : null
  const failedAtCreate = failure && failureStep(failure) === null ? failure : null

  return (
    <section
      ref={layer}
      className="new-channel"
      aria-label="New channel"
      data-dropping={dropping || undefined}
    >
      <div className="new-channel-band" aria-hidden="true" />
      <div className="new-channel-rule" aria-hidden="true" />
      <div className="new-channel-scroll">
        <div className="new-channel-column">
          <header className="new-channel-head">
            <h2>New channel</h2>
            <p>
              {steps.length === 3 ? "Three" : "Five"} quick questions · click a ✓ line to
              change it · Esc to cancel
            </p>
            <Button
              className="new-channel-close"
              variant="plain"
              size="icon-sm"
              shape="pill"
              aria-label="Cancel new channel"
              onClick={() => dispatch(closeChannelCreation())}
            >
              <DesktopIcon name="close" />
            </Button>
          </header>
          {steps.map((step) =>
            step === asking ? (
              <Asked key={step} step={step} flow={flow} failure={failedHere} />
            ) : flow.answered.includes(step) ? (
              <AnsweredLine key={step} step={step} draft={draft} disabled={creating} />
            ) : null,
          )}
        </div>
      </div>
      <form
        className="new-channel-composer"
        onSubmit={(event) => {
          event.preventDefault()
          if (ready) void dispatch(createChannel())
        }}
      >
        {failedAtCreate ? (
          <StatusLabel tone="warning" role="status" className="new-channel-notice">
            {failureText(failedAtCreate)}
          </StatusLabel>
        ) : asking === "name" && !nameFits(draft.name) ? (
          <StatusLabel tone="warning" role="status" className="new-channel-notice">
            Names can be up to {channelRules.maxNameChars} characters.
          </StatusLabel>
        ) : asking === "notes" && !notesFit(draft.notes) ? (
          <StatusLabel tone="warning" role="status" className="new-channel-notice">
            Notes can be up to {channelRules.maxNotesBytes / 1024} KB.
          </StatusLabel>
        ) : null}
        <div className="new-channel-composer-row">
          {typed(asking) ? (
            <textarea
              ref={field}
              aria-label={asking === "name" ? "Channel name" : "Channel notes"}
              className="new-channel-field"
              data-step={asking}
              rows={1}
              placeholder={composerHints[asking]}
              value={asking === "name" ? draft.name : draft.notes}
              readOnly={creating}
              onChange={(event) =>
                dispatch(
                  draftChannel({
                    change:
                      asking === "name"
                        ? { name: event.target.value.replace(/\n/g, " ") }
                        : { notes: event.target.value },
                  }),
                )
              }
              onKeyDown={(event) => {
                // Return answers; Shift-Return is a new line in the notes.
                if (
                  event.key !== "Enter" ||
                  event.shiftKey ||
                  event.metaKey ||
                  event.ctrlKey ||
                  event.nativeEvent.isComposing
                )
                  return
                event.preventDefault()
                if (!event.repeat) dispatch(answerChannelStep())
              }}
            />
          ) : (
            <span
              className="new-channel-hint"
              // Return goes on from anywhere in the layer that is not a field.
              aria-hidden="true"
            >
              {landing
                ? "Adding what you dropped…"
                : asking
                  ? composerHints[asking]
                  : `Ready to make #${draft.name.trim()}`}
            </span>
          )}
          <span className="new-channel-count" aria-label="Questions answered">
            {flow.answered.filter((step) => steps.includes(step)).length}/{steps.length}
          </span>
          {creating ? (
            <LoadingText className="new-channel-creating">Creating</LoadingText>
          ) : (
            <Button
              type="submit"
              className="new-channel-create"
              variant="inverse"
              size="30"
              shape="pill"
              disabled={!ready}
            >
              Create channel
              <Kbd className="new-channel-kbd">{createChord}</Kbd>
            </Button>
          )}
        </div>
      </form>
      {dropping ? (
        <div className="new-channel-drop" aria-hidden="true">
          <span className="new-channel-drop-label">
            <DesktopIcon name="folderAdd" />
            Drop to add to the channel
          </span>
        </div>
      ) : null}
      <span className="sr-only" aria-live="polite">
        {dropping ? "Drop files or folders to add them to the channel" : ""}
      </span>
    </section>
  )
}

/**
 * Watches for files and folders dragged onto the window while `watching`:
 * answers whether a drag is over it now. What is dropped is added by the
 * command (`watchChannelDrops`), as a pick's answer is.
 */
function useDrops(watching: boolean): boolean {
  const dispatch = useWorkspaceDispatch()
  const [over, setOver] = useState(false)
  useEffect(() => {
    if (!watching) return
    const stop = dispatch(watchChannelDrops({ dragging: setOver }))
    return () => {
      stop()
      setOver(false)
    }
  }, [dispatch, watching])
  return watching && over
}

/**
 * Covers the session list and the panes while the flow is open: they stay
 * laid out beneath it, out of reach (`inert`), as under the Agents overview.
 */
function useCovers(layer: RefObject<HTMLElement | null>) {
  useLayoutEffect(() => {
    const content = layer.current
      ?.closest(".workspace")
      ?.querySelector<HTMLElement>(".workspace-content")
    if (!content || content.hasAttribute("inert")) return
    content.setAttribute("inert", "")
    return () => content.removeAttribute("inert")
  }, [layer])
}

/** A step answered: one line, which asks it again when clicked. */
function AnsweredLine({
  step,
  draft,
  disabled,
}: {
  step: ChannelStep
  draft: ChannelDraft
  disabled: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  return (
    <button
      type="button"
      className="new-channel-answered"
      data-step={step}
      disabled={disabled}
      onClick={() => dispatch(askChannelStep({ step }))}
    >
      <span className="new-channel-check" aria-hidden="true">
        ✓
      </span>
      <Answer step={step} draft={draft} />
    </button>
  )
}

/**
 * The keyboard goes back where it was when the flow opened, once it closes:
 * the + that opened it. WebKit does not focus a clicked button, so with
 * nothing focused it goes to the first section's +. A channel that opens
 * focuses its own composer after this.
 */
function useFocusReturns() {
  useLayoutEffect(() => {
    const active = document.activeElement
    const opener =
      active instanceof HTMLElement && active !== document.body
        ? active
        : document.querySelector<HTMLElement>(".workspace-section-add")
    return () => {
      if (opener?.isConnected) opener.focus({ preventScroll: true })
    }
  }, [])
}

/** What a folded line says of its step. */
function Answer({ step, draft }: { step: ChannelStep; draft: ChannelDraft }) {
  const { home } = useWorkspaceSelector(selectChannelMaking)
  switch (step) {
    case "name":
      return (
        <span>
          Called <em>#{draft.name.trim()}</em>
        </span>
      )
    case "folder":
      return draft.folder === null ? (
        <span>
          Works in <em>a folder made for it</em>
        </span>
      ) : (
        <span>
          Works in <code>{homeShort(draft.folder, home)}</code>
        </span>
      )
    case "model": {
      const words = modelWords(draft.model)
      return (
        <span>
          Starts on <em>{words.name}</em> · {words.provider}
        </span>
      )
    }
    case "notes":
      return draft.notes.trim() === "" ? (
        <span>No notes</span>
      ) : (
        <span className="new-channel-clipped">
          Notes <em>{draft.notes.trim().split("\n")[0]}</em>
        </span>
      )
    case "resources":
      return draft.resources.length === 0 ? (
        <span>No files or folders</span>
      ) : (
        <span>
          Has <em>{draft.resources.map(pathName).join(", ")}</em>
        </span>
      )
  }
}

/** The step being asked: its question, and what answers it here. */
function Asked({
  step,
  flow,
  failure,
}: {
  step: ChannelStep
  flow: ChannelCreation
  failure: ChannelCreation["failure"]
}) {
  return (
    <div className="new-channel-asked" data-step={step}>
      <p className="new-channel-question">{questions[step]}</p>
      {/* A Create on its way holds the draft as sent: nothing here changes it. */}
      <fieldset className="new-channel-answers" disabled={flow.creating}>
        {step === "folder" ? <FolderAnswer flow={flow} /> : null}
        {step === "model" ? <ModelAnswer flow={flow} /> : null}
        {step === "resources" ? <ResourcesAnswer flow={flow} /> : null}
      </fieldset>
      {failure ? (
        <StatusLabel tone="warning" role="status" className="new-channel-notice">
          {failureText(failure)}
        </StatusLabel>
      ) : null}
    </div>
  )
}

/** Go on to the next question: Return, from a step answered by a pick. */
function useReturnAnswers() {
  const dispatch = useWorkspaceDispatch()
  return (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Enter" || event.metaKey || event.ctrlKey || event.shiftKey) return
    // A button's own Return presses it; only the row's own answers.
    if (event.target !== event.currentTarget) return
    event.preventDefault()
    dispatch(answerChannelStep())
  }
}

function FolderAnswer({ flow }: { flow: ChannelCreation }) {
  const dispatch = useWorkspaceDispatch()
  const { home } = useWorkspaceSelector(selectChannelMaking)
  const channels = useWorkspaceSelector(selectAllChannels)
  const onKeyDown = useReturnAnswers()
  const { folder } = flow.draft
  const sharing = channelsIn(channels, folder)
  return (
    <div className="new-channel-picks" data-answers tabIndex={-1} onKeyDown={onKeyDown}>
      <div className="new-channel-pick-row">
        <Button
          variant="tinted"
          size="30"
          shape="pill"
          aria-pressed={folder === null}
          onClick={() => {
            dispatch(draftChannel({ change: { folder: null } }))
            dispatch(answerChannelStep())
          }}
        >
          Make one for me
        </Button>
        <Button
          variant="tinted"
          size="30"
          shape="pill"
          aria-pressed={folder !== null}
          onClick={() => void dispatch(chooseChannelFolder())}
        >
          <DesktopIcon name="folder" />
          {folder === null ? "Choose a folder…" : homeShort(folder, home)}
        </Button>
        {folder !== null ? (
          <Button
            variant="plain"
            size="30"
            shape="pill"
            onClick={() => dispatch(answerChannelStep())}
          >
            Use this folder
          </Button>
        ) : null}
      </div>
      {sharing.length > 0 ? (
        <p className="new-channel-aside">
          Also used by {sharing.map((channel) => `#${channel.name}`).join(", ")}. Their
          agents work in the same files.
        </p>
      ) : (
        <p className="new-channel-aside">
          {folder === null
            ? "Nessa makes a folder of its own for the channel."
            : "Agents in this channel work in this folder."}
        </p>
      )}
    </div>
  )
}

function ModelAnswer({ flow }: { flow: ChannelCreation }) {
  const dispatch = useWorkspaceDispatch()
  const onKeyDown = useReturnAnswers()
  return (
    <div className="new-channel-picks" data-answers tabIndex={-1} onKeyDown={onKeyDown}>
      <div className="new-channel-pick-row">
        <CatalogueModelPicker
          value={flow.draft.model}
          onChange={(model) => dispatch(draftChannel({ change: { model } }))}
          side="bottom"
          className="new-channel-model"
        />
        <Button
          variant="plain"
          size="30"
          shape="pill"
          onClick={() => dispatch(answerChannelStep())}
        >
          Use this model
        </Button>
      </div>
    </div>
  )
}

function ResourcesAnswer({ flow }: { flow: ChannelCreation }) {
  const dispatch = useWorkspaceDispatch()
  const { home } = useWorkspaceSelector(selectChannelMaking)
  const onKeyDown = useReturnAnswers()
  const { resources } = flow.draft
  return (
    <div
      className="new-channel-resources"
      data-answers
      tabIndex={-1}
      onKeyDown={onKeyDown}
    >
      {resources.length > 0 ? (
        <ul role="list">
          {resources.map((path) => (
            <li key={path} data-kind={flow.kinds[path] ?? "file"}>
              <DesktopIcon name={flow.kinds[path] === "folder" ? "folder" : "file"} />
              <span className="new-channel-resource-name">{pathName(path)}</span>
              <code className="new-channel-resource-path">{homeShort(path, home)}</code>
              <Button
                variant="plain"
                size="icon-sm"
                shape="pill"
                aria-label={`Remove ${pathName(path)}`}
                onClick={() => dispatch(removeChannelResource({ path }))}
              >
                <DesktopIcon name="close" />
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      <div className="new-channel-pick-row">
        <Button
          variant="tinted"
          size="30"
          shape="pill"
          onClick={() => void dispatch(addChannelResources({ kind: "files" }))}
        >
          <DesktopIcon name="add" />
          Add files…
        </Button>
        <Button
          variant="tinted"
          size="30"
          shape="pill"
          onClick={() => void dispatch(addChannelResources({ kind: "folders" }))}
        >
          <DesktopIcon name="folderAdd" />
          Add folders…
        </Button>
      </div>
      {flow.leftOut > 0 ? (
        <StatusLabel tone="neutral" role="status" className="new-channel-notice">
          {flow.leftOut === 1 ? "One didn’t" : `${flow.leftOut} didn’t`} fit: a channel
          holds up to {channelRules.maxResources} files and folders.
        </StatusLabel>
      ) : null}
    </div>
  )
}
