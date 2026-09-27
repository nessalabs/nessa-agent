import { describe, expect, it } from "vitest"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { scenarioEffects } from "../scenario/effects"
import type { ModelCatalog } from "../../model"
import {
  bindConversation,
  catalogLoaded,
  chooseModel,
  restoreConversations,
} from "./slice"

const catalog: ModelCatalog = {
  defaultAgent: "claude",
  agents: [
    {
      agent: "claude",
      defaultModel: "claude-sonnet-5",
      approvalModes: ["ask"],
      models: [
        {
          modelId: "claude-sonnet-5",
          displayName: "Sonnet 5",
          maxContextWindowTokens: 1_000_000,
          reasoning: true,
          imageInput: true,
        },
      ],
    },
  ],
}

function store() {
  return makeStore(createDependencies({ conversation: scenarioEffects("echo") }))
}

describe("the model catalog in the store", () => {
  it("outlives restoring tabs, since it is not saved with them", () => {
    const state = store()
    state.dispatch(catalogLoaded(catalog))
    state.dispatch(restoreConversations({ tabs: [] }))

    expect(state.getState().conversation.catalog).toEqual(catalog)
  })
})

describe("starting to create a conversation", () => {
  it("fixes the model it is created with: the default when nobody chose", () => {
    const state = store()
    state.dispatch(catalogLoaded(catalog))
    const id = state.getState().conversation.activeId
    state.dispatch(bindConversation({ id, serverId: "server" }))

    expect(state.getState().conversation.conversations[0]!.modelChoice).toEqual({
      agent: "claude",
      model: "claude-sonnet-5",
    })
  })

  it("keeps a choice the catalog no longer lists out of the create", () => {
    const state = store()
    const id = state.getState().conversation.activeId
    state.dispatch(chooseModel({ id, choice: { agent: "claude", model: "retired" } }))
    state.dispatch(catalogLoaded(catalog))
    state.dispatch(bindConversation({ id, serverId: "server" }))

    expect(state.getState().conversation.conversations[0]!.modelChoice?.model).toBe(
      "claude-sonnet-5",
    )
  })
})
