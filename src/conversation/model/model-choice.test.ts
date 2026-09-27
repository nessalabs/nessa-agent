import { describe, expect, it } from "vitest"
import {
  approvalChoice,
  effectiveModel,
  modelChoiceOpen,
  type ModelCatalog,
} from "./model-choice"

const model = (modelId: string) => ({
  modelId,
  displayName: modelId,
  maxContextWindowTokens: 1_000_000,
  reasoning: true,
  imageInput: true,
})

const catalog: ModelCatalog = {
  defaultAgent: "codex",
  agents: [
    {
      agent: "claude",
      defaultModel: "claude-sonnet-5",
      approvalModes: ["ask"],
      models: [model("claude-sonnet-5"), model("claude-opus-5")],
    },
    {
      agent: "codex",
      defaultModel: "gpt-5.6-terra",
      approvalModes: ["ask", "auto", "full"],
      models: [model("gpt-5.6-terra"), model("gpt-6-astra")],
    },
  ],
}

const fresh = { turns: [] }
const sent = { turns: [{}] }

describe("whether a conversation's model can still be chosen", () => {
  it("is open until creation starts", () => {
    expect(modelChoiceOpen(fresh)).toBe(true)
    expect(modelChoiceOpen(sent)).toBe(false)
  })

  it("closes as soon as creation starts, before the gateway answers", () => {
    // An attachment staged before any message binds the tab and awaits the
    // create: a choice now would miss the create already in flight.
    expect(modelChoiceOpen({ turns: [], serverConversationId: "c0" })).toBe(false)
  })
})

describe("the model a conversation runs on or will be created with", () => {
  it("is what the gateway reports once it names the agent, over any choice", () => {
    expect(
      effectiveModel(
        {
          ...sent,
          modelChoice: { agent: "claude", model: "claude-opus-5" },
          remote: { runtime: { agent: "codex", model: "gpt-6-astra" } },
        },
        catalog,
      ),
    ).toEqual({ agent: "codex", model: "gpt-6-astra" })
  })

  it("is the choice made in a tab not yet created", () => {
    expect(
      effectiveModel(
        { ...fresh, modelChoice: { agent: "claude", model: "claude-opus-5" } },
        catalog,
      ),
    ).toEqual({ agent: "claude", model: "claude-opus-5" })
  })

  it("is the window's agent's default model when nothing was chosen", () => {
    expect(effectiveModel(fresh, catalog)).toEqual({
      agent: "codex",
      model: "gpt-5.6-terra",
    })
  })

  it("is the choice a conversation was created with until the gateway names its agent", () => {
    const created = {
      ...sent,
      modelChoice: { agent: "claude", model: "claude-opus-5" },
      remote: { runtime: { model: "gpt-6-astra" } },
    }
    expect(effectiveModel(created, catalog)).toEqual({
      agent: "claude",
      model: "claude-opus-5",
    })
  })

  it("is not named for a conversation this window did not create, until its view says", () => {
    expect(
      effectiveModel({ ...sent, remote: { runtime: { model: "gpt-6-astra" } } }, catalog),
    ).toBeUndefined()
  })

  it("is not named before a catalog", () => {
    expect(effectiveModel(fresh, undefined)).toBeUndefined()
  })

  it("is the first agent's default when the catalog does not list the window's agent", () => {
    expect(effectiveModel(fresh, { ...catalog, defaultAgent: "opencode" })).toEqual({
      agent: "claude",
      model: "claude-sonnet-5",
    })
  })

  it("drops a choice the catalog no longer lists while the tab can still choose", () => {
    const stale = { ...fresh, modelChoice: { agent: "claude", model: "retired" } }
    expect(effectiveModel(stale, catalog)).toEqual({
      agent: "codex",
      model: "gpt-5.6-terra",
    })
  })

  it("keeps the choice a conversation was created with, listed or not", () => {
    const creating = {
      turns: [],
      serverConversationId: "c0",
      modelChoice: { agent: "claude", model: "retired" },
    }
    expect(effectiveModel(creating, catalog)).toEqual({
      agent: "claude",
      model: "retired",
    })
  })
})

describe("the approval mode a new conversation starts in", () => {
  it("offers the modes the chosen agent honours, asking first by default", () => {
    expect(approvalChoice(fresh, catalog)).toEqual({
      mode: "ask",
      modes: ["ask", "auto", "full"],
    })
  })

  it("keeps a choice the agent honours", () => {
    expect(approvalChoice({ ...fresh, approvalChoice: "full" }, catalog)?.mode).toBe(
      "full",
    )
  })

  it("falls back to asking first when the chosen agent does not honour the choice", () => {
    expect(
      approvalChoice(
        {
          ...fresh,
          approvalChoice: "full",
          modelChoice: { agent: "claude", model: "claude-sonnet-5" },
        },
        catalog,
      ),
    ).toEqual({ mode: "ask", modes: ["ask"] })
  })

  it("offers nothing once the conversation exists, or before a catalog", () => {
    expect(approvalChoice(sent, catalog)).toBeUndefined()
    // Even when the gateway names an agent whose modes the catalog lists.
    expect(
      approvalChoice(
        { ...sent, remote: { runtime: { agent: "codex", model: "gpt-5.6-terra" } } },
        catalog,
      ),
    ).toBeUndefined()
    expect(approvalChoice(fresh, undefined)).toBeUndefined()
  })
})
