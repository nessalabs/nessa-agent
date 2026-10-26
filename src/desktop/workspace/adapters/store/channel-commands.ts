/**
 * The "New channel" flow's commands, re-exported by `commands.ts`, the one
 * list. The flow's rules are `usecases/channel-creation.ts`; these add what
 * it needs from outside: a fresh id, the system's pickers, and the source.
 * Each acts in the flow open now, by its opening, so an answer that comes
 * after it closed changes nothing. An agent drives it as a person does:
 *
 * ```ts
 * store.dispatch(openChannelCreation())
 * store.dispatch(draftChannel({ change: { name: "Design review" } }))
 * store.dispatch(answerChannelStep())
 * await store.dispatch(createChannel())
 * ```
 */
import { channelFailure } from "../../application/ports"
import { canCreate, type ChannelDraft, type ChannelStep } from "../../model/channel-draft"
import { defaultModel } from "../../model/workspace-index"
import { loadWorkspace, type WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const {
  channelCreationOpened,
  channelCreationClosed,
  channelDrafted,
  channelResourcesAdded,
  channelResourceRemoved,
  channelStepAsked,
  channelStepAnswered,
  channelCreating,
  channelCreationFailed,
  channelMade,
  channelHomeKnown,
} = workspaceActions

/** The flow's opening, when one is open. */
const openingOf = (state: {
  workspace: { channelCreation: { opening: string } | null }
}) => state.workspace.channelCreation?.opening

/**
 * Opens the flow, starting on the window's default model; nothing opens with
 * no model to start on. Where the window has pickers, the person's home
 * folder is asked for once, so the paths they pick read from `~`.
 */
export function openChannelCreation(): WorkspaceCommand {
  return (dispatch, getState, { newId, paths }) => {
    const model = defaultModel()
    if (!model) return
    dispatch(channelCreationOpened({ opening: newId(), model }))
    if (!paths || getState().workspace.channelMaking.home !== null) return
    void paths.home().then(
      (home) => {
        if (home) dispatch(channelHomeKnown({ home }))
      },
      (error: unknown) => console.warn("The home folder was not read", error),
    )
  }
}

/** Closes the flow (Esc, or its close button). */
export function closeChannelCreation(): WorkspaceCommand {
  return (dispatch, getState) => {
    const opening = openingOf(getState())
    if (opening) dispatch(channelCreationClosed({ opening }))
  }
}

/** Changes the draft: its name, folder, model, access or notes. */
export function draftChannel({
  change,
}: {
  change: Partial<Omit<ChannelDraft, "resources">>
}): WorkspaceCommand {
  return (dispatch, getState) => {
    const opening = openingOf(getState())
    if (opening) dispatch(channelDrafted({ opening, change }))
  }
}

/** Asks a step again: an answered line clicked. */
export function askChannelStep({ step }: { step: ChannelStep }): WorkspaceCommand {
  return (dispatch, getState) => {
    const opening = openingOf(getState())
    if (opening) dispatch(channelStepAsked({ opening, step }))
  }
}

/** Answers the step being asked and goes on to the next. */
export function answerChannelStep(): WorkspaceCommand {
  return (dispatch, getState) => {
    const opening = openingOf(getState())
    if (opening) dispatch(channelStepAnswered({ opening }))
  }
}

/** Takes one file or folder off the channel. */
export function removeChannelResource({ path }: { path: string }): WorkspaceCommand {
  return (dispatch, getState) => {
    const opening = openingOf(getState())
    if (opening) dispatch(channelResourceRemoved({ opening, path }))
  }
}

/** Asks the system's picker for the channel's folder; cancelled, nothing changes. */
export function chooseChannelFolder(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { paths }) => {
    const opening = openingOf(getState())
    if (!opening || !paths) return
    const folder = await paths.folder().catch((error: unknown) => {
      console.error("The folder picker failed", error)
      return null
    })
    if (folder !== null) dispatch(channelDrafted({ opening, change: { folder } }))
  }
}

/** Asks the system's picker for files, or folders, to attach; cancelled, nothing changes. */
export function addChannelResources({
  kind,
}: {
  kind: "files" | "folders"
}): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { paths }) => {
    const opening = openingOf(getState())
    if (!opening || !paths) return
    const picked = await paths.resources(kind).catch((error: unknown) => {
      console.error("The file picker failed", error)
      return [] as readonly string[]
    })
    const each = kind === "folders" ? "folder" : "file"
    dispatch(
      channelResourcesAdded({
        opening,
        added: picked.map((path) => ({ path, kind: each })),
      }),
    )
  }
}

/**
 * Watches for files and folders dragged onto the window while the flow asks
 * for them: `dragging` says when a drag is over it, and what is dropped is
 * added as a pick's answer is. A drop that lands once the question has moved
 * on, or the flow has closed, changes nothing. Answers how to stop watching;
 * where the window is never told of a drop, it watches nothing.
 */
export function watchChannelDrops({
  dragging,
}: {
  dragging: (over: boolean) => void
}): WorkspaceCommand<() => void> {
  return (dispatch, getState, { paths }) => {
    if (!paths?.drops) return () => {}
    return paths.drops({
      dragging,
      dropped: (added) => {
        const flow = getState().workspace.channelCreation
        if (!flow || flow.asking !== "resources") return
        dispatch(channelResourcesAdded({ opening: flow.opening, added }))
      },
    })
  }
}

/**
 * Makes the channel from the draft as the store holds it now. Made, it joins
 * the index and opens, and the index is read again so the source lists it
 * too. Refused, the flow says why at the step it is about. Returns the
 * channel's id, or nothing.
 */
export function createChannel(): WorkspaceCommand<Promise<string | undefined>> {
  return async (dispatch, getState, { channels, newId }) => {
    const flow = getState().workspace.channelCreation
    if (!flow || flow.creating || !channels || !canCreate(flow.draft)) return undefined
    const { opening, draft } = flow
    const requestId = flow.requestId ?? newId()
    dispatch(channelCreating({ opening, requestId }))
    try {
      const channel = await channels.create(draft, requestId)
      dispatch(channelMade({ opening, channel, draftId: newId() }))
      void dispatch(loadWorkspace())
      return channel.id
    } catch (error) {
      dispatch(channelCreationFailed({ opening, failure: channelFailure(error) }))
      return undefined
    }
  }
}
