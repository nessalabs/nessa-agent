import { expect, it, vi } from "vitest"
import { VaultChangeController } from "./change-controller"
import { VaultRefusal, type VaultSource, type VaultState } from "./ports"
function source(): VaultSource {
  return {
    status: vi.fn(async (): Promise<VaultState> => "unlocked"),
    setup: vi.fn(async (): Promise<VaultState> => "unlocked"),
    unlock: vi.fn(async (): Promise<VaultState> => "unlocked"),
    lock: vi.fn(async (): Promise<VaultState> => "locked"),
    changePassphrase: vi.fn(async (): Promise<VaultState> => "locked"),
  }
}
it("accepts only a sealed success, without reopening or resolving protected use", async () => {
  const backend = source()
  const controller = new VaultChangeController(backend)
  controller.open()
  await controller.submit("synthetic replacement phrase")
  expect(controller.snapshot()).toEqual({ kind: "finished" })
  expect(backend.unlock).not.toHaveBeenCalled()
  expect(backend.status).not.toHaveBeenCalled()
  await controller.submit("synthetic replacement phrase")
  expect(backend.changePassphrase).toHaveBeenCalledTimes(1)
})
it.each([
  "vault_change_unchanged",
  "vault_change_unchanged_audit_unavailable",
  "vault_change_not_confirmed",
  "vault_change_not_confirmed_audit_unavailable",
  "vault_changed_audit_unavailable",
] as const)("retains %s and refuses automatic resubmission", async (code) => {
  const backend = source()
  vi.mocked(backend.changePassphrase).mockRejectedValue(new VaultRefusal(code))
  const controller = new VaultChangeController(backend)
  controller.open()
  await controller.submit("synthetic replacement phrase")
  expect(controller.snapshot()).toEqual({ kind: "finished", failure: code })
  await controller.submit("synthetic replacement phrase")
  expect(backend.changePassphrase).toHaveBeenCalledTimes(1)
  expect(backend.status).not.toHaveBeenCalled()
})
it("keeps a policy refusal before admission retryable and never stores input", async () => {
  const backend = source()
  vi.mocked(backend.changePassphrase).mockRejectedValueOnce(
    new VaultRefusal("vault_passphrase_policy"),
  )
  const controller = new VaultChangeController(backend)
  controller.open()
  await controller.submit("synthetic replacement phrase")
  expect(controller.snapshot()).toEqual({
    kind: "form",
    busy: false,
    failure: "vault_passphrase_policy",
  })
  expect(JSON.stringify(controller.snapshot())).not.toContain("synthetic")
  await controller.submit("different synthetic replacement phrase")
  expect(controller.snapshot()).toEqual({ kind: "finished" })
})
it.each([new Error("lost response"), new VaultRefusal("vault_change_not_confirmed")])(
  "treats unknown delivery as recovery instead of unchanged",
  async (error) => {
    const backend = source()
    vi.mocked(backend.changePassphrase).mockRejectedValue(error)
    const controller = new VaultChangeController(backend)
    controller.open()
    await controller.submit("synthetic replacement phrase")
    expect(controller.snapshot()).toEqual({
      kind: "finished",
      failure: "vault_change_not_confirmed",
    })
  },
)
it("coalesces busy clicks and ignores completion after unmount", async () => {
  const backend = source()
  let resolve!: (state: VaultState) => void
  vi.mocked(backend.changePassphrase).mockReturnValue(
    new Promise((done) => {
      resolve = done
    }),
  )
  const controller = new VaultChangeController(backend)
  controller.open()
  const pending = controller.submit("synthetic replacement phrase")
  controller.close()
  await controller.submit("duplicate synthetic phrase")
  expect(controller.snapshot()).toEqual({ kind: "form", busy: true })
  controller.dispose()
  resolve("locked")
  await pending
  expect(controller.snapshot()).toEqual({ kind: "closed" })
  expect(backend.changePassphrase).toHaveBeenCalledTimes(1)
})
