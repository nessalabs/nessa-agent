import { describe, expect, it, vi } from "vitest"
import { BoatSignInFailure, boatClaudeSignIn, signInAction } from "./boat-sign-in"

describe("Boat sign-in", () => {
  it("is offered only for Claude whose refused run was in a Boat VM", () => {
    expect(signInAction("claude", "boat")).toBe("boat")
    for (const environment of ["here", "ssh", "peer", undefined] as const)
      expect(signInAction("claude", environment)).toBe("computer")
    expect(signInAction("codex", "boat")).toBe("computer")
  })

  it("hands the minted token straight to the gateway", async () => {
    const save = vi.fn(async () => ({ state: "connected" }))
    const signIn = boatClaudeSignIn({
      available: async () => true,
      mint: async () => "sk-ant-oat01-fixture",
      save,
    })
    await signIn.signIn()
    expect(save).toHaveBeenCalledWith("sk-ant-oat01-fixture")
  })

  it("says which stage failed and saves nothing it did not mint", async () => {
    const save = vi.fn(async () => undefined)
    const notMinted = boatClaudeSignIn({
      available: async () => true,
      mint: async () => {
        throw new Error("not-completed")
      },
      save,
    })
    await expect(notMinted.signIn()).rejects.toMatchObject({ stage: "minted" })
    expect(save).not.toHaveBeenCalled()
    const notSaved = boatClaudeSignIn({
      available: async () => true,
      mint: async () => "sk-ant-oat01-fixture",
      save: async () => {
        throw new Error("vault_locked")
      },
    })
    const failure = await notSaved.signIn().catch((error: unknown) => error)
    expect(failure).toBeInstanceOf(BoatSignInFailure)
    expect(failure).toMatchObject({ stage: "saved" })
  })

  it("is unavailable where the host cannot say", async () => {
    const signIn = boatClaudeSignIn({
      available: async () => {
        throw new Error("no host")
      },
      mint: async () => "",
      save: async () => undefined,
    })
    await expect(signIn.available()).resolves.toBe(false)
  })
})
