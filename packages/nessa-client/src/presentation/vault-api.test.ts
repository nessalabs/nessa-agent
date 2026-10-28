import { describe, it, expect, vi } from "vitest"
import { createVaultApi, vaultStatus } from "./vault-api.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { NessaVaultError } from "../application/vault-error.js"
describe("local vault controls", () => {
  it("uses the authenticated requester without exposing entries", async () => {
    const request = vi.fn(async () => ({ state: "unlocked" }))
    const api = createVaultApi({ request })
    await api.setup("private phrase words")
    await api.unlock("private phrase words")
    await api.lock()
    expect(request.mock.calls).toEqual([
      ["vault.setup", { passphrase: "private phrase words" }],
      ["vault.unlock", { passphrase: "private phrase words" }],
      ["vault.lock", {}],
    ])
  })
  it("refuses extra fields and inherited state", () => {
    expect(() => vaultStatus({ state: "unlocked", key: "secret" })).toThrow()
    expect(() => vaultStatus(Object.create({ state: "unlocked" }))).toThrow()
    expect(() => vaultStatus({ state: "unknown" })).toThrow()
  })
  it("returns redacted typed refusals without retaining the secret or arbitrary server text", async () => {
    const api = createVaultApi({
      request: vi.fn(async () => {
        throw new NessaRpcError("vault_wrong_passphrase", "private phrase words")
      }),
    })
    try {
      await api.unlock("private phrase words")
      throw new Error("expected refusal")
    } catch (error) {
      expect(error).toBeInstanceOf(NessaVaultError)
      expect((error as NessaVaultError).refusal).toBe("vault_wrong_passphrase")
      expect(String(error)).not.toContain("private phrase")
    }
  })
})
