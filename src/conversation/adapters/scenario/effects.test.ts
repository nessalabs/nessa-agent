import { expect, it } from "vitest"
import {
  ConversationReadFailedError,
  SubmissionRefusedError,
} from "../../application/ports"
import { scenarioEffects } from "./effects"

/**
 * The substitute is held to the same port the gateway adapter implements.
 *
 * A substitute that answers a failed read with something the port does not
 * promise is a substitute the panel could not have been written against: the
 * store's own fallback would absorb the difference and every test would stay
 * green while the two adapters disagreed.
 */
it("refuses a read the offline scenario cannot serve in the port's own words", async () => {
  const error = await scenarioEffects("offline")
    .read("server")
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(ConversationReadFailedError)
  expect(error).toMatchObject({ reason: "unavailable" })
  // The scenario's own sentence is still the cause, so a developer reading the
  // console learns which backend refused and why.
  expect((error as Error).cause).toMatchObject({ message: "Scenario: backend offline" })
})

it("refuses a read of a conversation the echo scenario never opened the same way", async () => {
  const effects = scenarioEffects("echo")
  const error = await effects.read("never-created").catch((error: unknown) => error)
  expect(error).toBeInstanceOf(ConversationReadFailedError)
  expect(error).toMatchObject({ reason: "unavailable" })
  // And a conversation it did open still reads, so the guard is about the
  // failure and not about refusing everything.
  await effects.create("server")
  expect(await effects.read("server")).toMatchObject({ conversationId: "server" })
})

it("still refuses a message the client would not put on the wire", async () => {
  // The other half of the same contract, and the reason this substitute exists:
  // it asks the client the same question the gateway adapter does.
  const error = await scenarioEffects("echo")
    .send({
      conversationId: "server",
      executionId: "execution",
      actionId: "action",
      text: "look",
      attachments: [{ digest: "sha256:NOPE", mimeType: "image/png", size: 1 }],
      files: [],
    })
    .catch((error: unknown) => error)
  expect(error).toBeInstanceOf(SubmissionRefusedError)
  expect(error).toMatchObject({ reason: "invalid-request" })
})
