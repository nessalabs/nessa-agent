import { expect, it, vi } from "vitest"
import { signOutBrowserSession } from "./browser-session"
it("discards local content before remote acknowledgement, including an uncertain failure", async () => {
  let reject!: (reason: Error) => void
  const pending = new Promise<void>((_, fail) => {
    reject = fail
  })
  const dispose = vi.fn()
  const result = signOutBrowserSession(dispose, () => pending)
  expect(dispose).toHaveBeenCalledOnce()
  reject(new Error("acknowledgement lost"))
  await expect(result).rejects.toThrow("acknowledgement lost")
})
