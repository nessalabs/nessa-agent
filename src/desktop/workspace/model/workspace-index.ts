/**
 * How work is organised: sections hold channels, and channels hold sessions,
 * each a conversation with one agent on one model. These are what the
 * workspace shows; the source that supplies them owns them.
 */
import { agentChoice, type AgentId } from "../../../onboarding/model/onboarding"
import {
  agentForProvider,
  composerModels,
  defaultComposerModel,
} from "../../model/composer-options"

export interface Section {
  readonly id: string
  readonly name: string
}

export interface Channel {
  readonly id: string
  readonly name: string
  readonly sectionId: string
  /** Only its members see it; drawn with a lock. */
  readonly private: boolean
  readonly topic: string
}

/**
 * Where a session stands: an agent working, a question or approval waiting
 * on the person, or nothing happening.
 */
export type SessionStatus = "running" | "needs-you" | "idle"

/** A model in the SDK's catalogue, as the composer names it. */
export interface ModelRef {
  readonly provider: string
  readonly modelId: string
}

export interface SessionSummary {
  readonly id: string
  readonly channelId: string
  readonly title: string
  /** The model the session runs on; its provider decides the agent. */
  readonly model: ModelRef
  readonly status: SessionStatus
  /** When it began, in epoch milliseconds. */
  readonly startedAt: number
  /** When anything last happened in it; lists sort by it. */
  readonly updatedAt: number
  /** The last thing said, as the session list previews it. */
  readonly preview: string
  /**
   * What is going on in it now, in one line of the source's own words —
   * "Running the reconnect tests after adding a token bucket" — as the
   * source summarises its turn; absent when the source has nothing to say,
   * and then nothing is shown — nor for a line of only whitespace, which
   * says nothing either (`nowLine`, the one rule). The window never writes
   * one: it is the source's, replaced with the summary it arrives in, at its
   * revision, and kept as it was sent.
   */
  readonly now?: string
  readonly pinned: boolean
  readonly unread: boolean
  /** The source's count of changes to this summary; see `revision.ts`. */
  readonly revision: number
}

export interface WorkspaceIndex {
  readonly sections: readonly Section[]
  readonly channels: readonly Channel[]
  readonly sessions: readonly SessionSummary[]
}

/** What an index said that contradicts the rest of it, by id: left out. */
export interface IndexContradictions {
  readonly sections: readonly string[]
  readonly channels: readonly string[]
  readonly sessions: readonly string[]
}

/**
 * The index as the workspace takes it: one section, channel or session per
 * id — the first listed — each channel under a section the index lists, and
 * each session in a channel it keeps. The index comes from outside the
 * window, and a channel under no section, or a session in no channel, would
 * be held but reachable from nowhere the window shows; an id listed twice
 * would have two owners. What is left out is said (`contradictions`), never
 * guessed at.
 */
export function consistentIndex(index: WorkspaceIndex): {
  readonly index: WorkspaceIndex
  readonly contradictions: IndexContradictions
} {
  const firsts = <T extends { readonly id: string }>(
    items: readonly T[],
    belongs: (item: T) => boolean,
  ) => {
    const kept = new Map<string, T>()
    const left: string[] = []
    for (const item of items)
      if (!kept.has(item.id) && belongs(item)) kept.set(item.id, item)
      else left.push(item.id)
    return { kept, left }
  }
  const sections = firsts(index.sections, () => true)
  const channels = firsts(index.channels, (channel) =>
    sections.kept.has(channel.sectionId),
  )
  const sessions = firsts(index.sessions, (session) =>
    channels.kept.has(session.channelId),
  )
  return {
    index: {
      sections: [...sections.kept.values()],
      channels: [...channels.kept.values()],
      sessions: [...sessions.kept.values()],
    },
    contradictions: {
      sections: sections.left,
      channels: channels.left,
      sessions: sessions.left,
    },
  }
}

/** Whether an index contradicts itself anywhere. */
export const contradicts = (contradictions: IndexContradictions): boolean =>
  contradictions.sections.length > 0 ||
  contradictions.channels.length > 0 ||
  contradictions.sessions.length > 0

/** The agent that runs a model, by its provider; Claude for a provider nessa runs no agent for. */
export function agentOf(model: ModelRef): AgentId {
  return agentForProvider(model.provider) ?? "claude"
}

/** The agent's name as people say it. */
export function agentName(agent: AgentId): string {
  return agentChoice(agent)?.name ?? agent
}

/** The model's name in the catalogue, or its id when the catalogue has no such model. */
export function modelName(model: ModelRef): string {
  return (
    composerModels.find(
      (entry) => entry.provider === model.provider && entry.modelId === model.modelId,
    )?.displayName ?? model.modelId
  )
}

/**
 * The source's line of what is going on in a session, as it sent it, when
 * it says something; `null` when it is absent or only whitespace. The one
 * rule for whether there is a line to show.
 */
export function nowLine(session: SessionSummary): string | null {
  return session.now !== undefined && session.now.trim() !== "" ? session.now : null
}

/** Newest first. */
export function byRecency(a: SessionSummary, b: SessionSummary): number {
  return b.updatedAt - a.updatedAt
}

/**
 * The model a new session starts on: the composer's default
 * (`defaultComposerModel`, the one owner of that choice), or nothing when
 * the catalogue has no model at all — then no new session starts.
 */
export function defaultModel(): ModelRef | undefined {
  const model = defaultComposerModel(composerModels)
  return model && { provider: model.provider, modelId: model.modelId }
}
