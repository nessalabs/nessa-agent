/**
 * The "New channel" flow, as pure transitions over the workspace. It opens
 * over the content region, asks its steps one at a time (`channelSteps`), and
 * a Create asks the source to make the channel; once made, the channel joins
 * the index and opens, with a new session's home in it.
 *
 * Every transition names the opening it acts in, so an answer that comes
 * after the flow closed, or opened again, changes nothing. While a Create is
 * on its way the draft is held: what was sent is what the source answers.
 */
import {
  channelStepsFor,
  channelName,
  emptyDraft,
  failureStep,
  withResources,
  type ChannelDraft,
  type ChannelFailure,
  type ChannelStep,
  type ResourceKind,
} from "../../model/channel-draft"
import type { Channel, ModelRef } from "../../model/workspace-index"
import { channelOf, type ChannelCreation, type WorkspaceState } from "../workspace-state"
import { navigated, openChannel } from "./navigation"

/** The flow, when it is the opening named and not held by a Create on its way. */
function editable(state: WorkspaceState, opening: string): ChannelCreation | null {
  const flow = state.channelCreation
  return flow && flow.opening === opening && !flow.creating ? flow : null
}

/** The first step this window asks and has not had answered, in order; null when all are. */
function nextAsked(
  state: WorkspaceState,
  answered: readonly ChannelStep[],
): ChannelStep | null {
  return (
    channelStepsFor(state.channelMaking.pickable).find(
      (step) => !answered.includes(step),
    ) ?? null
  )
}

/** Whether a step's answer, as the draft holds it, is one it can be left at. */
function answerable(draft: ChannelDraft, step: ChannelStep): boolean {
  return step === "name" ? channelName(draft.name) !== null : true
}

/**
 * Opens the flow on an empty draft starting on `model`, asking its name,
 * over the panes: the overview or a widget over them closes. A window that
 * cannot make channels opens nothing.
 */
export function channelCreationOpened(
  state: WorkspaceState,
  { opening, model }: { opening: string; model: ModelRef },
): WorkspaceState {
  if (!state.channelMaking.creatable) return state
  return {
    ...state,
    content: "panes",
    channelCreation: {
      opening,
      draft: emptyDraft(model),
      asking: "name",
      answered: [],
      requestId: null,
      creating: false,
      failure: null,
      leftOut: 0,
      kinds: {},
      landing: [],
    },
  }
}

/** Closes the flow, whatever it was doing; a Create on its way still settles at the source. */
export function channelCreationClosed(
  state: WorkspaceState,
  { opening }: { opening: string },
): WorkspaceState {
  return state.channelCreation?.opening === opening
    ? { ...state, channelCreation: null }
    : state
}

/**
 * Changes the draft. A different draft is another channel: the last
 * Create's id and refusal are let go. Resources are added through
 * `channelResourcesAdded`, which keeps the limits.
 */
export function channelDrafted(
  state: WorkspaceState,
  {
    opening,
    change,
  }: { opening: string; change: Partial<Omit<ChannelDraft, "resources">> },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow) return state
  return {
    ...state,
    channelCreation: {
      ...flow,
      draft: { ...flow.draft, ...change },
      requestId: null,
      failure: null,
    },
  }
}

/**
 * Adds picked or dropped files and folders, each once, as many as fit; says
 * how many did not. Each names its own kind: one drop can hold both.
 */
export function channelResourcesAdded(
  state: WorkspaceState,
  {
    opening,
    added,
  }: {
    opening: string
    added: readonly { readonly path: string; readonly kind: ResourceKind }[]
  },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow || added.length === 0) return state
  const { resources, leftOut } = withResources(
    flow.draft.resources,
    added.map(({ path }) => path),
  )
  const kinds = { ...flow.kinds }
  for (const { path, kind } of added)
    if (resources.includes(path) && !Object.hasOwn(kinds, path)) kinds[path] = kind
  return {
    ...state,
    channelCreation: {
      ...flow,
      draft: { ...flow.draft, resources },
      requestId: null,
      failure: null,
      leftOut,
      kinds,
    },
  }
}

/**
 * A drop landed, named `drop`; its paths come later. It is the flow's only
 * while the flow asks for files: landed on any other question, it is let go.
 */
export function channelDropLanded(
  state: WorkspaceState,
  { opening, drop }: { opening: string; drop: string },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow || flow.asking !== "resources" || flow.landing.includes(drop)) return state
  return {
    ...state,
    channelCreation: { ...flow, landing: [...flow.landing, drop] },
  }
}

/**
 * A drop's paths came. They are added, as a pick's are, only when the drop
 * landed on this flow and the flow still asks for files, and the `unread`
 * ones the host never looked at count as not fitting; either way the drop
 * stops holding Create.
 */
export function channelDropSettled(
  state: WorkspaceState,
  {
    drop,
    added,
    unread,
  }: {
    drop: string
    added: readonly { readonly path: string; readonly kind: ResourceKind }[]
    unread: number
  },
): WorkspaceState {
  const flow = state.channelCreation
  if (!flow || !flow.landing.includes(drop)) return state
  const settled: WorkspaceState = {
    ...state,
    channelCreation: {
      ...flow,
      landing: flow.landing.filter((each) => each !== drop),
    },
  }
  if (flow.asking !== "resources") return settled
  const kept = channelResourcesAdded(settled, { opening: flow.opening, added })
  const now = kept.channelCreation
  return unread > 0 && now
    ? {
        ...kept,
        channelCreation: {
          ...now,
          leftOut: (added.length > 0 ? now.leftOut : 0) + unread,
        },
      }
    : kept
}

/** Takes one file or folder off the channel. */
export function channelResourceRemoved(
  state: WorkspaceState,
  { opening, path }: { opening: string; path: string },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow || !flow.draft.resources.includes(path)) return state
  return {
    ...state,
    channelCreation: {
      ...flow,
      draft: {
        ...flow.draft,
        resources: flow.draft.resources.filter((resource) => resource !== path),
      },
      requestId: null,
      failure: null,
      leftOut: 0,
    },
  }
}

/** Asks a step again: an answered line clicked. */
export function channelStepAsked(
  state: WorkspaceState,
  { opening, step }: { opening: string; step: ChannelStep },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow || flow.asking === step) return state
  // The step being left keeps its answer only where it is one it can be left at.
  const left = flow.asking
  const answered =
    left && answerable(flow.draft, left) && !flow.answered.includes(left)
      ? [...flow.answered, left]
      : flow.answered
  return { ...state, channelCreation: { ...flow, asking: step, answered } }
}

/** Answers the step being asked, as the draft holds it, and asks the next one not answered. */
export function channelStepAnswered(
  state: WorkspaceState,
  { opening }: { opening: string },
): WorkspaceState {
  const flow = editable(state, opening)
  const step = flow?.asking
  if (!flow || !step || !answerable(flow.draft, step)) return state
  const answered = flow.answered.includes(step) ? flow.answered : [...flow.answered, step]
  return {
    ...state,
    channelCreation: { ...flow, asking: nextAsked(state, answered), answered },
  }
}

/** A Create on its way, under `requestId`: the one kept for this draft, or a fresh one. */
export function channelCreating(
  state: WorkspaceState,
  { opening, requestId }: { opening: string; requestId: string },
): WorkspaceState {
  const flow = editable(state, opening)
  if (!flow || channelName(flow.draft.name) === null) return state
  return {
    ...state,
    channelCreation: { ...flow, requestId, creating: true, failure: null },
  }
}

/**
 * A Create made nothing. The refusal is said at the step it is about, which
 * is asked again; one about no step is said beside Create. The request id is
 * kept: the same draft sent again is the same channel.
 */
export function channelCreationFailed(
  state: WorkspaceState,
  { opening, failure }: { opening: string; failure: ChannelFailure },
): WorkspaceState {
  const flow = state.channelCreation
  if (!flow || flow.opening !== opening || !flow.creating) return state
  const step = failureStep(failure)
  return {
    ...state,
    channelCreation: {
      ...flow,
      creating: false,
      failure,
      asking: step ?? flow.asking,
      answered: step
        ? flow.answered.filter((answered) => answered !== step)
        : flow.answered,
    },
  }
}

/**
 * The channel was made: it joins the index, the flow closes, and the channel
 * opens with a new session's home in it (`openChannel`). A later read of the
 * index lists it too. Made for a flow since closed, it still joins the
 * index, but nothing opens and the view stays where it is.
 */
export function channelMade(
  state: WorkspaceState,
  { opening, channel, draftId }: { opening: string; channel: Channel; draftId: string },
): WorkspaceState {
  const listed = channelOf(state, channel.id)
    ? state
    : { ...state, channels: [...state.channels, channel] }
  const flow = state.channelCreation
  if (!flow || flow.opening !== opening) return listed
  // Going to it is going somewhere: the panes, out from under the overview or a widget.
  return navigated(
    openChannel({ ...listed, channelCreation: null }, { channelId: channel.id, draftId }),
  )
}
