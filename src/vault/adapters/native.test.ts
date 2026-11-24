import { describe, expect, it, vi } from "vitest"
const host = vi.hoisted(() => ({ vaultCommand: vi.fn(), vaultBiometrics: vi.fn() }))
vi.mock("../../host", () => host)
import { nativeVault } from "./native"
describe("native vault transport", () => {
  it("coalesces mount replay reads without replaying passphrase commands", async () => {
    let resolve!: (value: unknown) => void
    host.vaultCommand.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      }),
    )
    const first = nativeVault.status()
    const second = nativeVault.status()
    expect(second).toBe(first)
    expect(host.vaultCommand).toHaveBeenCalledTimes(1)
    resolve({ ok: true, status: { state: "unlocked" } })
    expect(await first).toBe("unlocked")
    host.vaultCommand.mockResolvedValue({ ok: true, status: { state: "unlocked" } })
    await nativeVault.unlock("private passphrase words")
    await nativeVault.unlock("private passphrase words")
    expect(host.vaultCommand).toHaveBeenCalledTimes(3)
  })
  it("drops arbitrary native failure messages", async () => {
    host.vaultCommand.mockResolvedValue({ ok: false, code: "bad private payload" })
    await expect(nativeVault.status()).rejects.toThrow("vault_unavailable")
  })
  it("does not replay a change after native timeout or malformed response", async () => {
    host.vaultCommand.mockClear()
    host.vaultCommand.mockRejectedValueOnce(new Error("synthetic lost response"))
    await expect(
      nativeVault.changePassphrase("synthetic replacement phrase"),
    ).rejects.toThrow("vault_change_not_confirmed")
    host.vaultCommand.mockResolvedValueOnce({ ok: true, status: { state: "unknown" } })
    await expect(
      nativeVault.changePassphrase("synthetic replacement phrase"),
    ).rejects.toThrow("vault_change_not_confirmed")
    expect(host.vaultCommand).toHaveBeenCalledTimes(2)
  })
  it.each([
    ["vault_busy", "vault_busy"],
    ["forbidden", "vault_forbidden"],
    ["vault_unavailable", "vault_unavailable"],
  ])("preserves known pre-dispatch rejection %s", async (native, expected) => {
    host.vaultCommand.mockClear()
    host.vaultCommand.mockRejectedValueOnce(native)
    await expect(
      nativeVault.changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ code: expected })
    expect(host.vaultCommand).toHaveBeenCalledTimes(1)
  })
  it.each([
    null,
    { ok: true },
    { ok: true, status: { state: "unlocked" } },
    { ok: true, status: { state: "locked", key: "synthetic" } },
    { ok: false, code: "vault_busy", status: { state: "locked" } },
    { ok: true, status: { state: "locked" }, code: "vault_busy" },
  ])("rejects malformed or unsealed change success without replay", async (reply) => {
    host.vaultCommand.mockClear()
    host.vaultCommand.mockResolvedValueOnce(reply)
    await expect(
      nativeVault.changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ code: "vault_change_not_confirmed" })
    expect(host.vaultCommand).toHaveBeenCalledTimes(1)
  })
  it("rejects a prototype-supplied refusal discriminator", async () => {
    const reply = Object.assign(Object.create({ ok: false }), {
      code: "vault_busy",
      extra: true,
    })
    host.vaultCommand.mockResolvedValueOnce(reply)
    await expect(
      nativeVault.changePassphrase("synthetic replacement phrase"),
    ).rejects.toMatchObject({ code: "vault_change_not_confirmed" })
  })
})
describe("native Touch ID transport", () => {
  const biometrics = nativeVault.biometrics!
  it("asks the host to remember only when told to", async () => {
    host.vaultCommand.mockClear()
    host.vaultCommand.mockResolvedValue({ ok: true, status: { state: "unlocked" } })
    await nativeVault.unlock("private passphrase words")
    await nativeVault.unlock("private passphrase words", true)
    expect(host.vaultCommand.mock.calls).toEqual([
      [{ operation: "unlock", passphrase: "private passphrase words" }],
      [{ operation: "unlock", passphrase: "private passphrase words", remember: true }],
    ])
  })
  it("reads only two booleans as status, and nothing kept where unavailable", async () => {
    host.vaultBiometrics.mockResolvedValueOnce({ available: true, saved: true })
    expect(await biometrics.status()).toEqual({ available: true, saved: true })
    host.vaultBiometrics.mockResolvedValueOnce({ available: false, saved: true })
    expect(await biometrics.status()).toEqual({ available: false, saved: false })
    for (const odd of [undefined, { available: true }, { available: "yes", saved: true }])
      host.vaultBiometrics.mockResolvedValueOnce(odd)
    for (let i = 0; i < 3; i += 1)
      expect(await biometrics.status()).toEqual({ available: false, saved: false })
    host.vaultBiometrics.mockRejectedValueOnce("forbidden")
    expect(await biometrics.status()).toEqual({ available: false, saved: false })
  })
  it("answers a fingerprint unlock as the vault's reply, and keeps only known refusals", async () => {
    host.vaultBiometrics.mockResolvedValueOnce({
      ok: true,
      status: { state: "unlocked" },
    })
    expect(await biometrics.unlock()).toBe("unlocked")
    host.vaultBiometrics.mockResolvedValueOnce({
      ok: false,
      code: "vault_wrong_passphrase",
    })
    await expect(biometrics.unlock()).rejects.toMatchObject({
      code: "vault_wrong_passphrase",
    })
    host.vaultBiometrics.mockRejectedValueOnce("biometric_cancelled")
    await expect(biometrics.unlock()).rejects.toMatchObject({
      code: "biometric_cancelled",
    })
    host.vaultBiometrics.mockRejectedValueOnce("a private native message")
    await expect(biometrics.unlock()).rejects.toMatchObject({ code: "vault_unavailable" })
  })
})
