import { describe, expect, it, vi } from "vitest"

import { installDevConsoleForwarding } from "./dev-console"

describe("development page console forwarding", () => {
  it.each(["warn", "error"] as const)(
    "keeps console.%s visible in the page and forwards its value with a source",
    async (level) => {
      const original = vi.fn()
      const target = {
        warn: level === "warn" ? original : vi.fn(),
        error: level === "error" ? original : vi.fn(),
      }
      const forward = vi.fn()
      const restore = installDevConsoleForwarding(target, forward)

      target[level]("React said", { key: "c0" })

      expect(original).toHaveBeenCalledWith("React said", { key: "c0" })
      expect(forward).toHaveBeenCalledWith({
        level,
        message: 'React said {"key":"c0"}',
        source: expect.stringContaining("dev-console.test.ts"),
      })

      restore()
      expect(target[level]).toBe(original)
    },
  )

  it("does not let a refused terminal write replace the page warning", async () => {
    const original = vi.fn()
    const target = { warn: original, error: vi.fn() }
    const forward = vi.fn(async () => {
      throw new Error("terminal unavailable")
    })
    const restore = installDevConsoleForwarding(target, forward)
    const hostile = {
      toJSON: () => {
        throw new Error("no JSON")
      },
      toString: () => {
        throw new Error("no string")
      },
    }

    expect(() => target.warn("still visible", hostile)).not.toThrow()
    await Promise.resolve()
    expect(original).toHaveBeenCalledWith("still visible", hostile)
    expect(forward).toHaveBeenCalledWith(
      expect.objectContaining({ message: "still visible [unprintable value]" }),
    )

    restore()
  })
})
