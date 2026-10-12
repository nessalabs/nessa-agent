import { describe, expect, it } from "vitest"
import type { AgentsApi, AgentsListResult } from "@nessa/client"
import { nativeHooksGateway, type NativeHooksClient } from "./native-hooks-gateway"

function client(agents: Pick<AgentsApi, "list" | "setNativeHooks">): NativeHooksClient {
  return { agents }
}

function listed(value: AgentsListResult): Promise<AgentsListResult> {
  return Promise.resolve(value)
}

describe("nativeHooksGateway", () => {
  it("reads the stored choice even when Claude is not in the catalog", async () => {
    const gateway = nativeHooksGateway({
      connected: () =>
        Promise.resolve(
          client({
            list: () =>
              listed({
                environments: [],
                nativeHooks: { claude: "on", codex: "off" },
                agents: [],
              }),
            setNativeHooks: () => Promise.reject(new Error("unused")),
          }),
        ),
    })
    await expect(gateway.read()).resolves.toEqual({
      ok: true,
      choice: {
        claude: true,
        codex: false,
        claudeConfigured: false,
        codexConfigured: false,
      },
    })
  })

  it("sends the harness and reports a refusal without a new choice", async () => {
    const sent: unknown[] = []
    const gateway = nativeHooksGateway({
      connected: () =>
        Promise.resolve(
          client({
            list: () => Promise.reject(new Error("down")),
            setNativeHooks: (agent, enabled, requestId) => {
              sent.push({ agent, enabled, requestId })
              return Promise.resolve({ requestId, agent, enabled: false })
            },
          }),
        ),
    })
    await expect(gateway.read()).resolves.toEqual({ ok: false })
    await expect(gateway.set("codex", true, "attempt")).resolves.toEqual({
      ok: true,
      agent: "codex",
      enabled: false,
    })
    expect(sent).toEqual([{ agent: "codex", enabled: true, requestId: "attempt" }])
  })
})
