import { describe, expect, it, vi } from "vitest"

import { NessaLoginsError, LoginsRefusalCode } from "../application/logins-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { createLoginsApi } from "./logins-api.js"

function session(answer: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown }[] = []
  const request = vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params })
    return answer(method, params)
  })
  return { api: createLoginsApi({ request }), calls }
}

const peer = `peer:${"ab".repeat(32)}`
const status = {
  logins: [
    { agent: "claude", saved: true },
    { agent: "codex", saved: false },
    { agent: "opencode", saved: false },
  ],
  devicesOn: [peer],
}

describe("client.logins", () => {
  it("asks each method with its params and reads the status it answers", async () => {
    const { api, calls } = session(() => status)
    expect(await api.status()).toEqual(status)
    await api.save("codex", "sk-1")
    await api.remove("claude")
    await api.setDevice(peer, false)
    expect(calls).toEqual([
      { method: "logins.status", params: {} },
      { method: "logins.save", params: { agent: "codex", login: "sk-1" } },
      { method: "logins.remove", params: { agent: "claude" } },
      { method: "logins.setDevice", params: { environment: peer, on: false } },
    ])
  })

  it("refuses an answer outside the schema, a login above all", async () => {
    const [first, ...rest] = status.logins
    for (const answer of [
      { ...status, login: "sk-1" },
      { ...status, logins: [{ ...first, login: "sk-1" }, ...rest] },
      { ...status, logins: rest },
      { ...status, logins: [rest[0], first, rest[1]] },
      { ...status, devicesOn: ["studio"] },
      { ...status, devicesOn: [peer, peer] },
    ]) {
      const { api } = session(() => answer)
      const error = await api.status().catch((caught: unknown) => caught)
      expect(error).toBeInstanceOf(NessaLoginsError)
      expect((error as NessaLoginsError).refusal).toBeUndefined()
    }
  })

  it("names a refusal this build knows", async () => {
    const { api } = session(() => {
      throw new NessaRpcError("vault_locked", "refused")
    })
    const error = await api.save("claude", "x").catch((caught: unknown) => caught)
    expect((error as NessaLoginsError).refusal).toBe(LoginsRefusalCode.VaultLocked)
    expect((error as NessaLoginsError).method).toBe("logins.save")
  })
})
