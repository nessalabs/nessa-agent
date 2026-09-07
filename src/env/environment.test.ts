import { describe, expect, it } from "vitest"
import { loadEnvironment } from "./environment"
import { createDependencies } from "../composition/dependencies"
import { makeStore } from "../store"
import { sendDraft } from "../conversation/adapters/store/slice"

describe("frontend environment", () => {
  it("defaults to the real backend", () => {
    expect(loadEnvironment({}).conversation.backend).toBe("local")
  })
  it.each(["prod", "alpha"])("rejects scenarios in %s", (stage) => {
    expect(() =>
      loadEnvironment(
        {
          VITE_NESSA_STAGE: stage,
          VITE_NESSA_CONVERSATION_BACKEND: "scenario",
          VITE_NESSA_CONVERSATION_SCENARIO: "echo",
        },
        true,
      ),
    ).toThrow()
  })
  it("rejects dev scenarios in production builds", () => {
    expect(() =>
      loadEnvironment({
        VITE_NESSA_STAGE: "dev",
        VITE_NESSA_CONVERSATION_BACKEND: "scenario",
        VITE_NESSA_CONVERSATION_SCENARIO: "echo",
      }),
    ).toThrow()
  })
  it.each([
    { VITE_NESSA_STAGE: "typo" },
    { VITE_NESSA_CONVERSATION_BACKEND: "typo" },
    { VITE_NESSA_CONVERSATION_SCENARIO: "echo" },
    { VITE_NESSA_CONVERSATION_BACKEND: "scenario" },
    {
      VITE_NESSA_CONVERSATION_BACKEND: "scenario",
      VITE_NESSA_CONVERSATION_SCENARIO: "typo",
    },
  ])("rejects malformed selection %j", (source) => {
    expect(() => loadEnvironment(source, true)).toThrow()
  })
  it.each(["echo", "offline"])("composes %s without a socket", async (scenario) => {
    const dependencies = createDependencies({
      environment: loadEnvironment(
        {
          VITE_NESSA_STAGE: "ci",
          VITE_NESSA_CONVERSATION_BACKEND: "scenario",
          VITE_NESSA_CONVERSATION_SCENARIO: scenario,
        },
        true,
      ),
    })
    expect(dependencies.usesLocalSession).toBe(false)
    const result = await makeStore(dependencies).dispatch(sendDraft({ text: "hello" }))
    expect(sendDraft.fulfilled.match(result)).toBe(scenario === "echo")
    expect(dependencies.session.get()).toBeNull()
  })
})
