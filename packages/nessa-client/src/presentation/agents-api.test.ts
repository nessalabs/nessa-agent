import { bounds } from "../generated/product.js"
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

it("requests pinned installation with an explicit invocation and long download timeout", async () => {
  const result = {
    agent: "claude",
    version: "1.0",
    downloaded: true,
    cleanupPending: false,
  }
  const request = vi.fn().mockResolvedValue(result)
  const api = createAgentsApi({ request })
  expect(await api.install("claude", "invocation")).toEqual(result)
  expect(request).toHaveBeenCalledWith(
    "agents.install",
    { agent: "claude", requestId: "invocation" },
    { atLeastMs: 46 * 60 * 1000 },
  )
})

it.each([
  { agent: "unknown", version: "1", archiveBytes: 10, installed: false },
  { agent: "claude", version: "1", archiveBytes: -1, installed: false },
  { agent: "claude", version: "1", archiveBytes: 10, installed: "yes" },
])("rejects invalid download offers before display", async (offer) => {
  const api = createAgentsApi({ request: vi.fn().mockResolvedValue({ agents: [offer] }) })
  await expect(api.installOptions()).rejects.toThrow()
})

it("uses the published UTF-8 version bound for offers and results", async () => {
  const version = "é".repeat(bounds.maxAgentInstallVersionBytes / 2)
  const offer = { agent: "claude", version, archiveBytes: 1, installed: true }
  const result = { agent: "claude", version, downloaded: false, cleanupPending: false }
  const request = vi.fn().mockResolvedValue({ agents: [offer] })
  const api = createAgentsApi({ request })
  expect(await api.installOptions()).toEqual({ agents: [offer] })
  request.mockResolvedValue(result)
  expect(await api.install("claude", "bounded")).toEqual(result)
  request.mockResolvedValue({ ...result, version: version + "é" })
  await expect(api.install("claude", "oversized")).rejects.toThrow()
  request.mockResolvedValue({ agents: [{ ...offer, version: version + "é" }] })
  await expect(api.installOptions()).rejects.toThrow()
})
