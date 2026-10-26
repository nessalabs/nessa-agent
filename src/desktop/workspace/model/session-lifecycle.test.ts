import { describe, expect, it } from "vitest"
import { agentOf, modelName } from "./workspace-index"
import { keepShownDrafts, startSession, type Draft } from "./session-lifecycle"

const opus = { provider: "anthropic", modelId: "claude-opus-5" }
const draft = (id: string): Draft => ({ id, channelId: "desktop-app", model: opus })

describe("a draft's life", () => {
  it("keeps the drafts a pane still shows, and the same record when none is let go", () => {
    const drafts = { a: draft("a"), b: draft("b") }
    expect(keepShownDrafts(drafts, new Set(["a", "b", "other"]))).toBe(drafts)
    expect(keepShownDrafts(drafts, new Set(["b"]))).toEqual({ b: draft("b") })
    expect(keepShownDrafts(drafts, new Set())).toEqual({})
  })

  it("starts as a running session titled by its first message, keeping its identity", () => {
    expect(startSession(draft("a"), "fix the rain. Then the steam.", 42)).toEqual({
      id: "a",
      channelId: "desktop-app",
      title: "Fix the rain",
      model: opus,
      status: "running",
      startedAt: 42,
      updatedAt: 42,
      preview: "fix the rain. Then the steam.",
      pinned: false,
      unread: false,
      revision: 0,
    })
  })
})

describe("a session's model", () => {
  it("names the agent by the model's provider", () => {
    expect(agentOf(opus)).toBe("claude")
    expect(agentOf({ provider: "openai", modelId: "gpt-6-astra" })).toBe("codex")
    expect(agentOf({ provider: "opencode", modelId: "opencode/big-pickle" })).toBe(
      "opencode",
    )
    expect(agentOf({ provider: "elsewhere", modelId: "x" })).toBe("claude")
  })

  it("names the model from the catalogue, or by its id when the catalogue lacks it", () => {
    expect(modelName(opus)).toBe("Claude Opus 5")
    expect(modelName({ provider: "anthropic", modelId: "claude-unknown" })).toBe(
      "claude-unknown",
    )
  })
})
