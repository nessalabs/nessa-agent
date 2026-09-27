import { describe, expect, it, vi } from "vitest"
import { createAgentsApi } from "./agents-api.js"

const offered = {
  agents: [
    {
      agent: "claude",
      defaultModel: "claude-sonnet-5",
      models: [
        {
          modelId: "claude-sonnet-5",
          displayName: "Claude Sonnet 5",
          maxContextWindowTokens: 200000,
          reasoning: true,
          imageInput: true,
          approvalModes: [
            {
              id: "ask",
              name: "Provider asks",
              description: "The provider requests approval where required.",
            },
          ],
        },
      ],
    },
  ],
}

describe("authenticated agent choices", () => {
  it("reads and validates the gateway catalog", async () => {
    const request = vi.fn().mockResolvedValue(offered)
    const api = createAgentsApi({ request })
    expect(await api.list()).toEqual(offered)
    expect(request).toHaveBeenCalledWith("agents.list", {})
  })

  it.each([
    { ...offered, agents: [{ ...offered.agents[0], defaultModel: "absent" }] },
    {
      ...offered,
      agents: [
        {
          ...offered.agents[0],
          models: [{ ...offered.agents[0].models[0], approvalModes: [] }],
        },
      ],
    },
    { ...offered, agents: [...offered.agents, offered.agents[0]] },
  ])("rejects contradictory catalog facts before display", async (invalid) => {
    const api = createAgentsApi({ request: vi.fn().mockResolvedValue(invalid) })
    await expect(api.list()).rejects.toThrow()
  })
})
