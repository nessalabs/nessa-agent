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
import { savedLoginsGateway } from "./saved-logins-gateway"

const peer = `peer:${"cd".repeat(32)}`
const status: LoginsStatus = {
  logins: [
    { agent: "claude", saved: false },
    { agent: "codex", saved: true },
    { agent: "opencode", saved: false },
  ],
  devicesOn: [peer],
}

function over(answer: () => Promise<LoginsStatus>) {
  const logins: LoginsApi = {
    status: answer,
    save: answer,
    remove: answer,
    setDevice: answer,
  }
  return savedLoginsGateway(() => Promise.resolve({ logins }))
}

it("reads which agents have a login and which computers are on", async () => {
  const outcome = await over(() => Promise.resolve(status)).read()
  expect(outcome.kind).toBe("read")
  if (outcome.kind !== "read") return
  expect(outcome.logins.saved).toEqual({ claude: false, codex: true, opencode: false })
  expect([...outcome.logins.devicesOn]).toEqual([peer])
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
