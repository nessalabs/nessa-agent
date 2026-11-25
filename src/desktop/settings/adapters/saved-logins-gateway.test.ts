/**
 * The saved-logins gateway reads `client.logins` into the window's words and
 * gives every refusal its sentence.
 */
import { expect, it } from "vitest"
import {
  LoginsRefusalCode,
  NessaLoginsError,
  NessaRpcError,
  type LoginsApi,
  type LoginsStatus,
} from "@nessa/client"
import { chatgptEndSentence, savedLoginsGateway } from "./saved-logins-gateway"
import { ChatgptSignInEnd } from "@nessa/client"

const status: LoginsStatus = {
  logins: [
    { agent: "claude", saved: false },
    { agent: "codex", saved: true },
    { agent: "opencode", saved: false },
  ],
}

function over(answer: () => Promise<LoginsStatus>) {
  const logins: LoginsApi = {
    status: answer,
    save: answer,
    remove: answer,
    chatgptSignIn: answer,
    chatgptCancel: answer,
    chatgptUse: answer,
    chatgptRemove: answer,
  }
  return savedLoginsGateway(() => Promise.resolve({ logins }))
}

it("reads which agents have a login", async () => {
  const outcome = await over(() => Promise.resolve(status)).read()
  expect(outcome.kind).toBe("read")
  if (outcome.kind !== "read") return
  expect(outcome.logins.saved).toEqual({ claude: false, codex: true, opencode: false })
})

it("says every refusal in a sentence, and one for no answer", async () => {
  for (const code of Object.values(LoginsRefusalCode)) {
    const outcome = await over(() =>
      Promise.reject(
        new NessaLoginsError("logins.save", new NessaRpcError(code, "refused")),
      ),
    ).save("claude", "x")
    expect(outcome.kind).toBe("refused")
    if (outcome.kind === "refused") expect(outcome.sentence).not.toBe("")
  }
  const unread = await over(() => Promise.reject(new Error("gone"))).read()
  expect(unread).toEqual({
    kind: "refused",
    sentence: "The gateway didn’t answer. Try again.",
  })
})

const account = {
  id: "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e",
  email: "a@example.com",
  inUse: true,
  needsSignIn: false,
}

it("reads the ChatGPT accounts and where a sign-in stands", async () => {
  const waiting = await over(() =>
    Promise.resolve({
      ...status,
      chatgpt: {
        accounts: [account],
        signIn: {
          state: "waiting",
          attempt: "9e8d7c6b-5a49-4382-a716-152433425160",
          authorizeUrl: "https://auth.example/a",
        },
      },
    }),
  ).chatgptSignIn()
  expect(waiting.kind === "read" && waiting.logins.chatgpt).toEqual({
    accounts: [account],
    signIn: { kind: "waiting", authorizeUrl: "https://auth.example/a" },
  })
  const ended = await over(() =>
    Promise.resolve({
      ...status,
      chatgpt: {
        accounts: [],
        signIn: {
          state: "idle",
          attempt: "9e8d7c6b-5a49-4382-a716-152433425160",
          last: "timed_out",
        },
      },
    }),
  ).read()
  expect(ended.kind === "read" && ended.logins.chatgpt?.signIn).toEqual({
    kind: "idle",
    last: "timed_out",
  })
  const none = await over(() => Promise.resolve(status)).read()
  expect(none.kind === "read" && none.logins.chatgpt).toBeUndefined()
})

it("says how a sign-in ended, except when the person stopped it", () => {
  for (const end of Object.values(ChatgptSignInEnd)) {
    const sentence = chatgptEndSentence(end)
    if (end === "cancelled" || end === "replaced") expect(sentence).toBeUndefined()
    else expect(sentence).toMatch(/\.$/)
  }
})
