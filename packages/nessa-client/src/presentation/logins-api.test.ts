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

  const account = "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e"
  const chatgpt = {
    accounts: [{ id: account, email: "a@example.com", inUse: true, needsSignIn: false }],
    signIn: { state: "waiting", authorizeUrl: "https://auth.example/authorize?x=1" },
  }

  it("asks the ChatGPT methods with their params and reads the accounts", async () => {
    const { api, calls } = session(() => ({ ...status, chatgpt }))
    expect((await api.chatgptSignIn()).chatgpt).toEqual(chatgpt)
    await api.chatgptSignIn(account)
    await api.chatgptCancel()
    await api.chatgptUse(account)
    await api.chatgptUse(null)
    await api.chatgptRemove(account)
    expect(calls).toEqual([
      { method: "logins.chatgptSignIn", params: {} },
      { method: "logins.chatgptSignIn", params: { accountId: account } },
      { method: "logins.chatgptCancel", params: {} },
      { method: "logins.chatgptUse", params: { accountId: account } },
      { method: "logins.chatgptUse", params: { accountId: null } },
      { method: "logins.chatgptRemove", params: { accountId: account } },
    ])
  })

  it("refuses ChatGPT accounts outside the schema, a token above all", async () => {
    const [first] = chatgpt.accounts
    for (const bad of [
      { ...chatgpt, accounts: [{ ...first, access: "eyJ.a.b" }] },
      { ...chatgpt, accounts: [{ ...first, id: "one" }] },
      { ...chatgpt, accounts: [first, first] },
      { ...chatgpt, accounts: [first, { ...first, id: account.replace("0b", "0c") }] },
      {
        ...chatgpt,
        accounts: Array.from({ length: 9 }, (_, index) => ({
          ...first,
          inUse: false,
          id: account.replace("0b", index.toString(16).padStart(2, "0")),
        })),
      },
      { ...chatgpt, signIn: { state: "idle", authorizeUrl: "https://x" } },
      { ...chatgpt, signIn: { state: "idle", last: "gone" } },
      { ...chatgpt, signIn: { state: "waiting" } },
      { ...chatgpt, signIn: { state: "waiting", authorizeUrl: "javascript:alert(1)" } },
      { ...chatgpt, token: "x" },
    ]) {
      const { api } = session(() => ({ ...status, chatgpt: bad }))
      const error = await api.status().catch((caught: unknown) => caught)
      expect(error).toBeInstanceOf(NessaLoginsError)
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
