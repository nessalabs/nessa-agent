import { expect, it } from "vitest"
import { sha256Digest } from "./sha256"

it("identifies bytes the way the contract spells it: sha256: and lowercase hex", async () => {
  // Published SHA-256 test vectors, so this cannot agree with itself by accident.
  expect(await sha256Digest(new Blob(["abc"]))).toBe(
    "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  )
  expect(await sha256Digest(new Blob([]))).toBe(
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  )
})

it("hashes the bytes, not the type or the way they were split", async () => {
  const whole = await sha256Digest(new Blob([new Uint8Array([0, 255, 16, 1])]))
  const parts = await sha256Digest(
    new Blob([new Uint8Array([0, 255]), new Uint8Array([16, 1])], { type: "image/png" }),
  )
  expect(parts).toBe(whole)
  expect(whole).toMatch(/^sha256:[0-9a-f]{64}$/)
})
