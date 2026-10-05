import { describe, expect, it } from "vitest"
import { deviceKeyFingerprint } from "./device-key"

describe("deviceKeyFingerprint", () => {
  it("is SHA-256 of the Ed25519 SubjectPublicKeyInfo, not the raw key", async () => {
    const key = new Uint8Array(32).fill(1)
    await expect(deviceKeyFingerprint(key)).resolves.toBe(
      "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7",
    )
  })

  it("refuses a key that is not 32 bytes", async () => {
    await expect(deviceKeyFingerprint(new Uint8Array(31))).rejects.toThrow(/32/)
  })
})
