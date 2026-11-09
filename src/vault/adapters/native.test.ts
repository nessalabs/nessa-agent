import { describe, expect, it, vi } from "vitest"
const host = vi.hoisted(() => ({ vaultCommand: vi.fn() }))
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
