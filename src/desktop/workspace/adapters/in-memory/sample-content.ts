/**
 * What the in-memory source's sample sessions are written with: the models
 * they run on, the parts an agent's message is made of, and the shape of one
 * sample session. The samples themselves are in `sample-starred.ts` and
 * `sample-labs.ts`; `sample-workspace.ts` turns them into an index.
 */
import type { ModelRef, SessionStatus } from "../../model/workspace-index"
import type { Approval, Part } from "../../model/transcript"

export const models = {
  opus: { provider: "anthropic", modelId: "claude-opus-5" },
  sonnet: { provider: "anthropic", modelId: "claude-sonnet-5" },
  fable: { provider: "anthropic", modelId: "claude-fable-5-1" },
  astra: { provider: "openai", modelId: "gpt-6-astra" },
  sol: { provider: "openai", modelId: "gpt-5.6-sol" },
  minimax: { provider: "opencode", modelId: "opencode/minimax-m3" },
  pickle: { provider: "opencode", modelId: "opencode/big-pickle" },
} satisfies Record<string, ModelRef>

export const text = (value: string): Part => ({ kind: "text", text: value })
export const read = (detail: string): Part => ({
  kind: "step",
  step: "read",
  label: "Read",
  detail,
})
export const ran = (detail: string): Part => ({
  kind: "step",
  step: "run",
  label: "Ran",
  detail,
})
export const searched = (detail: string): Part => ({
  kind: "step",
  step: "search",
  label: "Searched for",
  detail,
})
export const edited = (detail: string, added: number, removed?: number): Part => ({
  kind: "step",
  step: "edit",
  label: "Edited",
  detail,
  added,
  removed,
})

export interface SampleSession {
  readonly id: string
  readonly channelId: string
  readonly title: string
  readonly model: ModelRef
  readonly status?: SessionStatus
  /** Minutes since it last moved, and since it began. */
  readonly updated: number
  readonly started?: number
  readonly preview: string
  readonly pinned?: boolean
  readonly unread?: boolean
  /** What it is doing, and for how many seconds, while it runs. */
  readonly activity?: readonly [label: string, seconds: number]
  readonly approval?: Omit<Approval, "answering" | "failure">
  /** The conversation, each message with its minutes-ago. */
  readonly messages: readonly (readonly ["user" | "agent", number, readonly Part[]])[]
}

/** A prompt and its reply, the reply's steps first. */
export function exchange(
  prompt: string,
  reply: string,
  at: number,
  steps: readonly Part[] = [],
): SampleSession["messages"] {
  return [
    ["user", at + 6, [text(prompt)]],
    ["agent", at, [...steps, text(reply)]],
  ]
}
