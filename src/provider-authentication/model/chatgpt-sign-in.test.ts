import { describe, expect, it, vi } from "vitest"
import type { LoginsStatus } from "@nessa/client"
import { ChatgptSignInFailure, codexChatgptSignIn } from "./chatgpt-sign-in"

const base: LoginsStatus = {
  logins: [
    { agent: "claude", saved: false },
    { agent: "codex", saved: false },
    { agent: "opencode", saved: false },
  ],
  devicesOn: [],
}
const waiting: LoginsStatus = {
  ...base,
  chatgpt: {
    accounts: [],
    signIn: { state: "waiting", authorizeUrl: "https://auth.example/a" },
  },
}
const idle = (last?: "signed_in" | "declined"): LoginsStatus => ({
  ...base,
  chatgpt: { accounts: [], signIn: { state: "idle", ...(last ? { last } : {}) } },
})

describe("ChatGPT sign-in for Codex", () => {
  it("is offered only where the gateway keeps ChatGPT accounts", async () => {
    const pause = async () => {}
    const begin = async () => waiting
    expect(
      await codexChatgptSignIn({ status: async () => idle(), begin, pause }).available(),
    ).toBe(true)
    expect(
      await codexChatgptSignIn({ status: async () => base, begin, pause }).available(),
    ).toBe(false)
    const gone = async (): Promise<LoginsStatus> => {
      throw new Error("gone")
    }
    expect(await codexChatgptSignIn({ status: gone, begin, pause }).available()).toBe(
      false,
    )
  })

  it("opens the browser on the sign-in and reads until it ends", async () => {
    const reads = [waiting, waiting, idle("signed_in")]
    const open = vi.fn()
    const opened = vi.fn()
    const pause = vi.fn(async () => {})
    const signIn = codexChatgptSignIn({
      status: async () => reads.shift() ?? idle(),
      begin: async () => waiting,
      open,
      pause,
      pollMs: 7,
    })
    expect(await signIn.signIn(opened)).toBe("signed_in")
    expect(opened).toHaveBeenCalledWith("https://auth.example/a")
    expect(open).toHaveBeenCalledWith("https://auth.example/a")
    expect(pause).toHaveBeenCalledTimes(3)
    expect(pause).toHaveBeenCalledWith(7)
  })

  it("says how one ended at once, and fails when it cannot start or be read", async () => {
    const pause = async () => {}
    expect(
      await codexChatgptSignIn({
        status: async () => idle(),
        begin: async () => idle("declined"),
        pause,
      }).signIn(() => {}),
    ).toBe("declined")
    await expect(
      codexChatgptSignIn({
        status: async () => idle(),
        begin: async () => {
          throw new Error("refused")
        },
        pause,
      }).signIn(() => {}),
    ).rejects.toBeInstanceOf(ChatgptSignInFailure)
    await expect(
      codexChatgptSignIn({
        status: async () => {
          throw new Error("gone")
        },
        begin: async () => waiting,
        pause,
      }).signIn(() => {}),
    ).rejects.toBeInstanceOf(ChatgptSignInFailure)
  })
})
