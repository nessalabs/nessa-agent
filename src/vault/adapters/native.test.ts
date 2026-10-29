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
})
