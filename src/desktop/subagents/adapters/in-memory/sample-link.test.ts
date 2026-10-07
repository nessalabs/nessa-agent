import { describe, expect, it } from "vitest"
import {
  labsSamples,
  retryBudgetSession,
} from "../../../workspace/adapters/in-memory/sample-labs"
import { subagentsPluginId } from "../../application/ports"

describe("the sample conversation's way into the panel", () => {
  it("carries a subagents widget for the session the sample source fills", () => {
    const session = labsSamples.find((each) => each.id === retryBudgetSession)
    const widgets = session?.messages.flatMap((message) =>
      message[2].filter((part) => part.kind === "widget"),
    )
    expect(widgets).toEqual([
      { kind: "widget", widget: { plugin: subagentsPluginId, id: retryBudgetSession } },
    ])
  })
})
