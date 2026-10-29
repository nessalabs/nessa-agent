/**
 * A channel being made: what the "New channel" flow asks, one question at a
 * time, before it asks the source to make it. A channel holds what a new
 * conversation in it starts with (its model and access), the folder its
 * agents work in, and notes and files every agent in it is told.
 *
 * The flow asks in a fixed order (`channelSteps`). An answered step folds into
 * a line that can be clicked to change it; the next unanswered one is asked.
 * Only the name must be given: a channel with no chosen folder gets one made
 * for it, and notes and resources may be left empty.
 */
import type { AccessMode } from "./access"
import type { Channel, ModelRef } from "./workspace-index"

/** The questions, in the order they are asked. */
export const channelSteps = [
  "name",
  "folder",
  "placement",
  "model",
  "notes",
  "resources",
] as const
export type ChannelStep = (typeof channelSteps)[number]

/**
 * The questions a window asks: all of them where it has the system's pickers;
 * without, no folder or resources, since only a picker names a path, and the
 * channel works in a folder made for it.
 */
export function channelStepsFor(pickable: boolean): readonly ChannelStep[] {
  return pickable
    ? channelSteps
    : channelSteps.filter((step) => step !== "folder" && step !== "resources")
}

/** What a resource is, as the picker that chose it said. */
export type ResourceKind = "file" | "folder"

/**
 * The protocol's bounds (`x-channelRules`), as the flow checks them before
 * asking. The model does not import the client, so the gateway adapter's
 * test holds these equal to the client's published `channelRules`.
 */
export const channelRules = {
  maxNameChars: 80,
  maxNotesBytes: 8_192,
  maxResources: 32,
  maxResourceBytes: 16_384,
  maxPathBytes: 4_096,
} as const

export interface ChannelDraft {
  readonly name: string
  /** A folder the person chose; null has the source make one. */
  readonly folder: string | null
  readonly model: ModelRef
  readonly access: AccessMode
  readonly coordinatorPlacement: "local" | "boat"
  readonly threadPlacement: "local" | "boat"
  readonly notes: string
  /** Absolute files and folders, in the order they were added, none twice. */
  readonly resources: readonly string[]
}

/** A draft before anything is answered, starting on `model`. */
export function emptyDraft(model: ModelRef): ChannelDraft {
  return {
    name: "",
    folder: null,
    coordinatorPlacement: "local",
    threadPlacement: "local",
    model,
    access: "ask",
    notes: "",
    resources: [],
  }
}

const bytes = (text: string) => new TextEncoder().encode(text).length

/** The name as it is kept: trimmed; null when it is not one a channel can have. */
export function channelName(text: string): string | null {
  const name = text.trim()
  // eslint-disable-next-line no-control-regex
  if (name === "" || /[\u0000-\u001f\u007f-\u009f]/.test(name)) return null
  return nameFits(name) ? name : null
}

/** Whether a name, once trimmed, is short enough: counted in characters, not UTF-16 units. */
export function nameFits(text: string): boolean {
  return [...text.trim()].length <= channelRules.maxNameChars
}

/** Whether the notes fit what a channel keeps. */
export function notesFit(notes: string): boolean {
  return bytes(notes) <= channelRules.maxNotesBytes
}

/**
 * `resources` with `added` after them: each path once, in order, as many as
 * fit. What did not fit is left out and counted.
 */
export function withResources(
  resources: readonly string[],
  added: readonly string[],
): { readonly resources: readonly string[]; readonly leftOut: number } {
  const kept = [...resources]
  let total = kept.reduce((sum, path) => sum + bytes(path), 0)
  let leftOut = 0
  for (const path of added) {
    if (kept.includes(path)) continue
    const size = bytes(path)
    if (
      kept.length >= channelRules.maxResources ||
      size > channelRules.maxPathBytes ||
      total + size > channelRules.maxResourceBytes
    ) {
      leftOut += 1
      continue
    }
    kept.push(path)
    total += size
  }
  return { resources: kept, leftOut }
}

/** Whether the draft can be made: it has a name, and notes that fit. */
export function canCreate(draft: ChannelDraft): boolean {
  return channelName(draft.name) !== null && notesFit(draft.notes)
}

/** The channels already working in `folder`, besides any being made. */
export function channelsIn(
  channels: readonly Channel[],
  folder: string | null,
): readonly Channel[] {
  return folder === null ? [] : channels.filter((channel) => channel.folder === folder)
}

/** The last part of a path, as a row names a file or folder. */
export function pathName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "")
  return trimmed.split(/[\\/]/).pop() || trimmed
}

/** A path under the person's home, written from `~`. */
export function homeShort(path: string, home: string | null): string {
  if (!home) return path
  const base = home.replace(/[\\/]+$/, "")
  return path === base || path.startsWith(`${base}/`) || path.startsWith(`${base}\\`)
    ? `~${path.slice(base.length)}`
    : path
}

/**
 * Why a source did not make a channel, by kind. The flow says each in its own
 * words, at the step it is about.
 */
export type ChannelFailure =
  | "folder-not-found"
  | "folder-not-a-directory"
  | "folder-refused"
  | "resource-not-found"
  | "limit"
  | "conflict"
  | "invalid"
  | "forbidden"
  | "unavailable"

/** The step a failure sends the person back to; null when it is no one step's. */
export function failureStep(failure: ChannelFailure): ChannelStep | null {
  switch (failure) {
    case "folder-not-found":
    case "folder-not-a-directory":
    case "folder-refused":
      return "folder"
    case "resource-not-found":
      return "resources"
    case "invalid":
    case "limit":
    case "conflict":
    case "forbidden":
    case "unavailable":
      return null
  }
}

/** What the flow says about a failure. */
export function failureText(failure: ChannelFailure): string {
  switch (failure) {
    case "folder-not-found":
      return "That folder isn’t there any more. Choose another, or have one made."
    case "folder-not-a-directory":
      return "That’s a file, not a folder. Choose a folder."
    case "folder-refused":
      return "Agents can’t work there. Choose a folder inside your projects instead."
    case "resource-not-found":
      return "One of the files or folders isn’t there any more. Remove it and try again."
    case "limit":
      return "You have as many channels as Nessa keeps."
    case "conflict":
      return "This channel was already made differently. Close this and start again."
    case "invalid":
      return "Something here isn’t one a channel can hold. Check the name and notes."
    case "forbidden":
      return "This window can’t make channels."
    case "unavailable":
      return "Nessa didn’t answer. Try again."
  }
}
