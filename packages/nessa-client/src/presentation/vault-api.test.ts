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
  it("sends change once and treats an unknown reply as an unconfirmed change", async () => {
    const request = vi.fn(async () => {
      throw new Error("private transport text")
    })
    const api = createVaultApi({ request })
    await expect(
      api.changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ refusal: "vault_change_not_confirmed" })
    expect(request.mock.calls).toEqual([
      ["vault.changePassphrase", { passphrase: "synthetic replacement phrase" }],
    ])
  })
  it("accepts only Locked as a confirmed passphrase-change success", async () => {
    const request = vi.fn(async () => ({ state: "locked" }))
    const api = createVaultApi({ request })
    expect(await api.changePassphrase("synthetic replacement phrase")).toEqual({
      state: "locked",
    })
    request.mockResolvedValueOnce({ state: "unlocked" })
    await expect(
      api.changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ refusal: "vault_change_not_confirmed" })
    expect(request).toHaveBeenCalledTimes(2)
  })
  it("preserves authorization refusal before passphrase change dispatch", async () => {
    const request = vi.fn(async () => {
      throw new NessaRpcError("forbidden", "synthetic private message")
    })
    await expect(
      createVaultApi({ request }).changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ refusal: "vault_forbidden" })
    expect(request).toHaveBeenCalledTimes(1)
  })
  it.each([
    "vault_change_unchanged",
    "vault_change_unchanged_audit_unavailable",
    "vault_change_not_confirmed",
    "vault_change_not_confirmed_audit_unavailable",
    "vault_changed_audit_unavailable",
  ])("preserves publication and audit result %s", async (code) => {
    const request = vi.fn(async () => {
      throw new NessaRpcError(code, "synthetic secret")
    })
    await expect(
      createVaultApi({ request }).changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ refusal: code })
    expect(request).toHaveBeenCalledTimes(1)
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
