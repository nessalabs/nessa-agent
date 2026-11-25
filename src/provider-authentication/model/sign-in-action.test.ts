import { expect, it } from "vitest"
import { signInAction } from "./sign-in-action"

it("offers ChatGPT only for Codex whose refused run was on another computer", () => {
  for (const environment of ["here", "ssh", "peer", "boat", undefined] as const)
    expect(signInAction("claude", environment)).toBe("computer")
  for (const environment of ["ssh", "peer", "boat"] as const)
    expect(signInAction("codex", environment)).toBe("chatgpt")
  for (const environment of ["here", undefined] as const)
    expect(signInAction("codex", environment)).toBe("computer")
})
