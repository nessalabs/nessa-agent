import { describe, it, expect, vi } from "vitest"
import { VaultController } from "./controller"
import { VaultRefusal, type VaultSource, type VaultState } from "./ports"
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
function source(state: VaultState): VaultSource {
  return {
    status: vi.fn(async () => state),
    setup: vi.fn(async (): Promise<VaultState> => "unlocked"),
    unlock: vi.fn(async (): Promise<VaultState> => "unlocked"),
    lock: vi.fn(async (): Promise<VaultState> => "locked"),
  }
}
describe("one reusable vault dialog", () => {
  it("coalesces concurrent requests and keeps passphrases out of its snapshot", async () => {
    const backend = source("unconfigured")
    const controller = new VaultController(backend)
    const first = controller.ensure()
    const second = controller.ensure()
    expect(second).toBe(first)
    await Promise.resolve()
    expect(controller.snapshot().kind).toBe("setup")
    const saving = deferred<VaultState>()
    vi.mocked(backend.setup).mockReturnValue(saving.promise)
    const submit = controller.submit("private test passphrase")
    expect(controller.snapshot()).toEqual({
      kind: "setup",
      busy: true,
      failure: undefined,
    })
    expect(JSON.stringify(controller.snapshot())).not.toContain("private test")
    saving.resolve("unlocked")
    await submit
    expect(await first).toBe(true)
    expect(await second).toBe(true)
    expect(controller.snapshot().kind).toBe("closed")
  })
  it("does not flash a modal for an already-unlocked source", async () => {
    const controller = new VaultController(source("unlocked"))
    const pending = controller.ensure()
    expect(controller.snapshot().kind).toBe("closed")
    expect(await pending).toBe(true)
  })
  it("cancels without invoking secret access and can be opened again", async () => {
    const backend = source("locked")
    const controller = new VaultController(backend)
    const first = controller.ensure()
    await Promise.resolve()
    controller.cancel()
    expect(await first).toBe(false)
    expect(backend.unlock).not.toHaveBeenCalled()
    const next = controller.ensure()
    await Promise.resolve()
    await controller.submit("test passphrase words")
    expect(await next).toBe(true)
  })
  it("drops a late status response after StrictMode cleanup and remount", async () => {
    const backend = source("locked")
    const old = deferred<VaultState>()
    vi.mocked(backend.status).mockReturnValueOnce(old.promise)
    const controller = new VaultController(backend)
    const pending = controller.ensure()
    controller.dispose()
    expect(await pending).toBe(false)
    controller.activate()
    const next = controller.ensure()
    await Promise.resolve()
    expect(controller.snapshot().kind).toBe("unlock")
    old.resolve("unlocked")
    await Promise.resolve()
    expect(controller.snapshot().kind).toBe("unlock")
    controller.cancel()
    expect(await next).toBe(false)
  })
  it("shows a typed refusal and switches to unlock after committed setup", async () => {
    const backend = source("unconfigured")
    const controller = new VaultController(backend)
    const pending = controller.ensure()
    await Promise.resolve()
    vi.mocked(backend.setup).mockRejectedValue(
      new VaultRefusal("vault_migration_unavailable"),
    )
    vi.mocked(backend.status).mockResolvedValue("locked")
    await controller.submit("test passphrase words")
    expect(controller.snapshot()).toEqual({
      kind: "unlock",
      busy: false,
      failure: "vault_migration_unavailable",
    })
    controller.cancel()
    expect(await pending).toBe(false)
  })
  it("ignores a focus read that finishes during unlock", async () => {
    const backend = source("locked")
    const controller = new VaultController(backend)
    const accepted = controller.ensure()
    await Promise.resolve()
    const read = deferred<VaultState>()
    vi.mocked(backend.status).mockReturnValueOnce(read.promise)
    controller.refreshPending()
    const unlock = deferred<VaultState>()
    vi.mocked(backend.unlock).mockReturnValueOnce(unlock.promise)
    const submitting = controller.submit("private test passphrase")
    read.resolve("locked")
    await Promise.resolve()
    expect(controller.snapshot()).toEqual({
      kind: "unlock",
      busy: true,
      failure: undefined,
    })
    unlock.resolve("unlocked")
    await submitting
    expect(await accepted).toBe(true)
  })
  it("does not reopen an old failed submit after remount completes", async () => {
    const backend = source("locked")
    const controller = new VaultController(backend)
    const old = controller.ensure()
    await Promise.resolve()
    const read = deferred<VaultState>()
    vi.mocked(backend.unlock).mockRejectedValueOnce(
      new VaultRefusal("vault_wrong_passphrase"),
    )
    vi.mocked(backend.status).mockReturnValueOnce(read.promise)
    const submitting = controller.submit("private test passphrase")
    await Promise.resolve()
    controller.dispose()
    controller.activate()
    vi.mocked(backend.status).mockResolvedValueOnce("unlocked")
    expect(await controller.ensure()).toBe(true)
    read.resolve("locked")
    await submitting
    expect(await old).toBe(false)
    expect(controller.snapshot()).toEqual({ kind: "closed" })
  })
})
