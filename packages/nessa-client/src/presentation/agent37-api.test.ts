import { describe, expect, it } from "vitest"
import { NessaRpcError } from "../application/rpc-error.js"
import { agent37Status, cloudChoice, createAgent37Api } from "./agent37-api.js"

describe("agent37 api", () => {
  it("reads only the statuses the gateway sends", () => {
    expect(agent37Status({ state: "locked" })).toEqual({ state: "locked" })
    expect(agent37Status({ state: "connected", template: "nessa-runtime@1" })).toEqual({
      state: "connected",
      template: "nessa-runtime@1",
    })
    for (const bad of [
      { state: "connected" },
      { state: "disconnected", template: "x" },
      { state: "connected", template: "x", key: "sk_live_leak" },
      null,
    ])
      expect(() => agent37Status(bad)).toThrow(TypeError)
    expect(cloudChoice({ cloud: "agent37" })).toEqual({ cloud: "agent37" })
    expect(() => cloudChoice({ cloud: "elsewhere" })).toThrow(TypeError)
  })

  it("sends the key once and reports a refusal by its code alone", async () => {
    const sent: [string, unknown][] = []
    const api = createAgent37Api({
      request: async (method: string, params: unknown) => {
        sent.push([method, params])
        if (method === "agent37.save")
          throw new NessaRpcError(
            "agent37_unavailable",
            "the key sk_live_secret was refused",
          )
        return { cloud: "agent37" }
      },
    } as never)
    await expect(api.save({ key: "sk_live_secret" })).rejects.toMatchObject({
      name: "NessaAgent37Error",
      refusal: "agent37_unavailable",
      message: "Agent37 operation refused (agent37_unavailable)",
    })
    expect(await api.chooseCloud("agent37")).toEqual({ cloud: "agent37" })
    expect(sent).toEqual([
      ["agent37.save", { key: "sk_live_secret" }],
      ["cloud.choose", { cloud: "agent37" }],
    ])
  })
})
