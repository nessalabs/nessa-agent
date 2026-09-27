import { describe, expect, it } from "vitest"
import {
  approvalChoice,
  effectiveModel,
  modelChoiceOpen,
  type ModelCatalog,
} from "./model-choice"

const catalog: ModelCatalog = {
  defaultAgent: "codex",
  agents: [
    {
      agent: "claude",
      defaultModel: "claude-sonnet-5",
      approvalModes: ["ask"],
      models: [],
    },
    {
      agent: "codex",
      defaultModel: "gpt-5.6-terra",
      approvalModes: ["ask", "auto", "full"],
      models: [],
    },
  ],
}

const fresh = { turns: [] }
const sent = { turns: [{}] }

describe("whether a conversation's model can still be chosen", () => {
  it("is open until the first send creates it", () => {
    expect(modelChoiceOpen(fresh)).toBe(true)
    expect(modelChoiceOpen(sent)).toBe(false)
    expect(modelChoiceOpen({ turns: [], serverReady: true })).toBe(false)
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

  it("is not named for a created conversation whose gateway does not say", () => {
    expect(
      effectiveModel(
        {
          ...sent,
          modelChoice: { agent: "claude", model: "claude-opus-5" },
          remote: { runtime: { model: "gpt-6-astra" } },
        },
        catalog,
      ),
    ).toBeUndefined()
  })

  it("is not named before a catalog, or when the window's agent is not in it", () => {
    expect(effectiveModel(fresh, undefined)).toBeUndefined()
    expect(
      effectiveModel(fresh, { ...catalog, defaultAgent: "opencode" }),
    ).toBeUndefined()
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
