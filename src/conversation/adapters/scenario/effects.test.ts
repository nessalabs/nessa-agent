import { expect, it } from "vitest"
import {
  ConversationReadFailedError,
  HeldFileReadError,
  SubmissionRefusedError,
  type ConversationEffects,
} from "../../application/ports"
import type { ConversationView } from "../../application/view"
import type { ReadFailure } from "../../model"
import { scenarioEffects } from "./effects"

/** What following `id` is told first: its view, or the word and cause for none. */
function firstWord(effects: ConversationEffects, id: string) {
  return new Promise<
    { view: ConversationView } | { failed: ReadFailure; cause: unknown }
  >((done) => {
    const stop = effects.follow(id, {
      view: (view) => {
        stop()
        done({ view })
      },
      failed: (reason, cause) => {
        stop()
        done({ failed: reason, cause })
      },
    })
  })
}

it("publishes complete history after echo admission and preserves it on replay", async () => {
  const effects = scenarioEffects("echo")
  const conversationId = "00000000-0000-4000-8000-000000000001"
  await effects.create(conversationId)
  const views: ConversationView[] = []
  const stop = effects.follow(conversationId, {
    view: (view) => views.push(view),
    failed: (reason) => {
      throw new Error(`no view: ${reason}`)
    },
  })
  await Promise.resolve()
  expect(views.at(-1)).toMatchObject({
    transcriptState: "complete_empty",
    messages: [],
    revision: "0",
  })
  const input = {
    conversationId,
    executionId: "execution-1",
    actionId: "action-1",
    text: "hello",
    attachments: [],
    files: [],
  }
  // Each change is told to the follower, a task later.
  await effects.send(input)
  await Promise.resolve()
  const first = views.at(-1)
  expect(first).toMatchObject({
    transcriptState: "complete",
    messages: [{ executionId: input.executionId, userText: input.text }],
    revision: "1",
  })
  await effects.send(input)
  await Promise.resolve()
  expect(views.at(-1)).toEqual(first)
  await effects.steer({
    ...input,
    executionId: "execution-2",
    actionId: "action-2",
    text: "again",
  })
  await Promise.resolve()
  expect(views.at(-1)).toMatchObject({
    transcriptState: "complete",
    messages: [
      { executionId: "execution-1", userText: "hello" },
      { executionId: "execution-2", userText: "again" },
    ],
    revision: "2",
  })
  stop()
  const told = views.length
  await effects.steer({ ...input, executionId: "execution-3", actionId: "action-3" })
  await Promise.resolve()
  // Nothing is told after the follow is stopped.
  expect(views).toHaveLength(told)
})

/**
 * The substitute is held to the same port the gateway adapter implements.
 *
 * A substitute that answers a failed read with something the port does not
 * promise is a substitute the panel could not have been written against: the
 * store's own fallback would absorb the difference and every test would stay
 * green while the two adapters disagreed.
 */
it("refuses a follow the offline scenario cannot serve in the port's own words", async () => {
  const said = await firstWord(scenarioEffects("offline"), "server")
  expect(said).toMatchObject({ failed: "unavailable" })
  // The scenario's own sentence is still the cause, so a developer reading the
  // console learns which backend refused and why.
  const cause = (said as { cause: unknown }).cause
  expect(cause).toBeInstanceOf(ConversationReadFailedError)
  expect((cause as Error).cause).toMatchObject({ message: "Scenario: backend offline" })
})

it("refuses a follow of a conversation the echo scenario never opened the same way", async () => {
  const effects = scenarioEffects("echo")
  expect(await firstWord(effects, "never-created")).toMatchObject({
    failed: "unavailable",
  })
  // And a conversation it did open is still followed, so the guard is about
  // the failure and not about refusing everything.
  await effects.create("server")
  expect(await firstWord(effects, "server")).toMatchObject({
    view: { conversationId: "server" },
  })
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

it("echoes the images it was sent back as published files it holds, until it closes", async () => {
  const effects = scenarioEffects("echo")
  const conversationId = "00000000-0000-4000-8000-000000000002"
  await effects.create(conversationId)
  const bytes = new Blob(["png"], { type: "image/png" })
  const file = { digest: `sha256:${"a".repeat(64)}`, mimeType: "image/png", size: 3 }
  const image = await effects.stageAttachment(
    conversationId,
    file,
    bytes,
    new AbortController().signal,
  )
  await effects.send({
    conversationId,
    executionId: "execution-1",
    actionId: "action-1",
    text: "look",
    attachments: [image],
    files: [],
  })
  const told = await firstWord(effects, conversationId)
  expect(told).toMatchObject({
    view: {
      artifacts: [{ executionId: "execution-1", name: "echo-1.png", ...file }],
    },
  })
  const signal = new AbortController().signal
  const read = await effects.readHeldFile(conversationId, file, signal)
  expect(await read.text()).toBe("png")
  expect(read.type).toBe("image/png")
  // Held as another type, it is another file, which nothing holds.
  await expect(
    effects.readHeldFile(conversationId, { ...file, mimeType: "image/jpeg" }, signal),
  ).rejects.toMatchObject({ reason: "not-held" })
  // Closing lets go of the bytes, as the gateway does; the record stays.
  await effects.close(conversationId)
  await expect(effects.readHeldFile(conversationId, file, signal)).rejects.toBeInstanceOf(
    HeldFileReadError,
  )
  await expect(effects.readHeldFile(conversationId, file, signal)).rejects.toMatchObject({
    reason: "not-held",
  })
})

it("lists the newest 64 files the echo agent published, as the gateway's view does", async () => {
  const effects = scenarioEffects("echo")
  const conversationId = "00000000-0000-4000-8000-000000000003"
  await effects.create(conversationId)
  const file = { digest: `sha256:${"b".repeat(64)}`, mimeType: "image/png", size: 3 }
  const image = await effects.stageAttachment(
    conversationId,
    file,
    new Blob(["png"], { type: "image/png" }),
    new AbortController().signal,
  )
  for (let turn = 1; turn <= 65; turn++)
    await effects.send({
      conversationId,
      executionId: `execution-${turn}`,
      actionId: `action-${turn}`,
      text: "again",
      attachments: [image],
      files: [],
    })
  const told = await firstWord(effects, conversationId)
  const artifacts = (told as { view: { artifacts: { executionId?: string }[] } }).view
    .artifacts
  expect(artifacts).toHaveLength(64)
  expect(artifacts[0]?.executionId).toBe("execution-2")
  expect(artifacts.at(-1)?.executionId).toBe("execution-65")
})
