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
const ours = "6c3f0a52-1d1e-4d0b-9a55-6b8f5a0c7e01"
const theirs = "1b2c3d4e-5f60-4a7b-8c9d-0e1f2a3b4c5d"
const waitingFor = (attempt: string): LoginsStatus => ({
  ...base,
  chatgpt: {
    accounts: [],
    signIn: { state: "waiting", attempt, authorizeUrl: "https://auth.example/a" },
  },
})
const waiting = waitingFor(ours)
const idle = (last?: "signed_in" | "declined", attempt = ours): LoginsStatus => ({
  ...base,
  chatgpt: {
    accounts: [],
    signIn: { state: "idle", ...(last ? { last, attempt } : {}) },
  },
})

/** The answer to `chatgptSignIn`: `status`, naming the attempt it started. */
const begun = (status: LoginsStatus, started = ours): LoginsStatus => ({
  ...status,
  chatgpt: { ...status.chatgpt!, started },
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
    const reads = [idle(), waiting, waiting, idle("signed_in")]
    const open = vi.fn()
    const opened = vi.fn()
    const pause = vi.fn(async () => {})
    const signIn = codexChatgptSignIn({
      status: async () => reads.shift() ?? idle(),
      begin: async () => begun(waiting),
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

  it("never reports another window's sign-in as its own", async () => {
    const pause = async () => {}
    for (const reads of [
      [idle(), waitingFor(theirs)],
      [idle(), idle("signed_in", theirs)],
    ]) {
      const signIn = codexChatgptSignIn({
        status: async () => reads.shift() ?? idle(),
        begin: async () => begun(waiting),
        pause,
      })
      expect(await signIn.signIn(() => {})).toBe("replaced")
    }
    // Another window's started just after this one: its sign-in is not ours
    // to open, even in the answer to ours.
    const opened = vi.fn()
    const signIn = codexChatgptSignIn({
      status: async () => idle(),
      begin: async () => begun(waitingFor(theirs)),
      pause,
    })
    expect(await signIn.signIn(opened)).toBe("replaced")
    expect(opened).not.toHaveBeenCalled()
  })

  it("signs in again the account in use when it needs it, else a new one", async () => {
    const pause = async () => {}
    const account = "2a1b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5e"
    const needing = (needsSignIn: boolean): LoginsStatus => ({
      ...base,
      chatgpt: {
        accounts: [{ id: account, email: "a@example.test", inUse: true, needsSignIn }],
        signIn: { state: "idle" },
      },
    })
    for (const [status, expected] of [
      [needing(true), account],
      [needing(false), undefined],
    ] as const) {
      const begin = vi.fn(async (_accountId?: string) => begun(idle("declined")))
      await codexChatgptSignIn({ status: async () => status, begin, pause }).signIn(
        () => {},
      )
      expect(begin).toHaveBeenCalledWith(expected)
    }
  })

  it("says how one ended at once, and fails when it cannot start or be read", async () => {
    const pause = async () => {}
    expect(
      await codexChatgptSignIn({
        status: async () => idle(),
        begin: async () => begun(idle("declined")),
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
        begin: async () => begun(waiting),
        pause,
      }).signIn(() => {}),
    ).rejects.toBeInstanceOf(ChatgptSignInFailure)
  })
})
