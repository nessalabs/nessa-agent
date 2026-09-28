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
  readonly pinned: boolean
  readonly unread: boolean
  /** The source's count of changes to this summary; see `revision.ts`. */
  readonly revision: number
}

export interface Overview {
  readonly sections: readonly Section[]
  readonly channels: readonly Channel[]
  readonly sessions: readonly SessionSummary[]
}

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
