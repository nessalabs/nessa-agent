import { describe, expect, it, vi } from "vitest"

import { GithubRefusalCode, NessaGithubError } from "../application/github-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { createGithubApi } from "./github-api.js"

const installUrl = "https://github.com/apps/nessa-agent/installations/new"

function session(answer: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown }[] = []
  const request = vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params })
    return answer(method, params)
  })
  return { api: createGithubApi({ request }), calls }
}

describe("client.github", () => {
  it("asks each method with an empty object and keeps the state's own parts", async () => {
    const { api, calls } = session((method) =>
      method === "github.connect"
        ? {
            state: "connecting",
            installUrl,
            code: {
              userCode: "ABCD-1234",
              verificationUri: "https://github.com/login/device",
              expiresInSeconds: 899,
            },
          }
        : method === "github.status"
          ? { state: "connected", installUrl, account: { id: 7, login: "octo" } }
          : { state: "signed_out", installUrl, last: "cancelled", unrecorded: true },
    )
    expect((await api.connect()).code?.userCode).toBe("ABCD-1234")
    expect((await api.status()).account).toEqual({ id: 7, login: "octo" })
    expect(await api.cancel()).toEqual({
      state: "signed_out",
      installUrl,
      last: "cancelled",
      unrecorded: true,
    })
    expect(await api.disconnect()).toEqual({
      state: "signed_out",
      installUrl,
      last: "cancelled",
      unrecorded: true,
    })
    expect(calls).toEqual([
      { method: "github.connect", params: {} },
      { method: "github.status", params: {} },
      { method: "github.cancel", params: {} },
      { method: "github.disconnect", params: {} },
    ])
  })

  it("does not believe an answer whose parts disagree with its state", async () => {
    for (const odd of [
      { state: "connected", installUrl },
      { state: "connecting", installUrl },
      { state: "signed_out", installUrl, account: { id: 1, login: "x" } },
      { state: "connected", installUrl, account: { id: 1, login: "x" }, last: "denied" },
      { state: "signed_out", installUrl, last: "later" },
      { state: "signed_out", installUrl, unrecorded: true },
      { state: "signed_out", installUrl, last: "denied", unrecorded: false },
      { state: "waiting", installUrl },
      { state: "signed_out" },
      { state: "signed_out", installUrl, token: "ghu_x" },
    ]) {
      const { api } = session(() => odd)
      await expect(api.status()).rejects.toBeInstanceOf(NessaGithubError)
    }
  })

  it("types a refusal this build knows, and leaves an unknown code untyped", async () => {
    const refused = session(() => {
      throw new NessaRpcError("github_unreachable", "offline")
    })
    const error = await refused.api.connect().catch((cause: unknown) => cause)
    expect(error).toBeInstanceOf(NessaGithubError)
    expect((error as NessaGithubError).refusal).toBe(GithubRefusalCode.GithubUnreachable)
    expect((error as NessaGithubError).method).toBe("github.connect")
    const unknown = session(() => {
      throw new NessaRpcError("github_new_thing", "?")
    })
    const untyped = await unknown.api.status().catch((cause: unknown) => cause)
    expect((untyped as NessaGithubError).refusal).toBeUndefined()
  })
})
