import { expect, it } from "vitest"
import type { VoiceKeyGateway, VoiceKeyOutcome } from "./voice-gateway"
import { watchedVoiceKey } from "./watched-voice-key"

/** A gateway whose answers are given by hand, in any order. */
function gateway() {
  const pending: { resolve: (outcome: VoiceKeyOutcome) => void }[] = []
  const ask = () => new Promise<VoiceKeyOutcome>((resolve) => pending.push({ resolve }))
  const fake: VoiceKeyGateway = { read: ask, save: ask, forget: ask }
  return { fake, pending }
}

const saved = (keySaved: boolean): VoiceKeyOutcome => ({ kind: "read", keySaved })
const refused: VoiceKeyOutcome = { kind: "refused", sentence: "Nessa is locked." }

it("knows nothing until the gateway says, then what it said last", async () => {
  const { fake, pending } = gateway()
  const key = watchedVoiceKey(fake)
  expect(key.known()).toBeNull()
  const read = key.read()
  pending[0].resolve(saved(false))
  await read
  expect(key.known()).toBe(false)
  const save = key.save("sk-test")
  pending[1].resolve(saved(true))
  await save
  expect(key.known()).toBe(true)
})

it("a read the gateway refused leaves the key unknown; a refused save changes nothing", async () => {
  const { fake, pending } = gateway()
  const key = watchedVoiceKey(fake)
  const read = key.read()
  pending[0].resolve(saved(true))
  await read
  const save = key.save("sk-test")
  pending[1].resolve(refused)
  await save
  expect(key.known()).toBe(true)
  const again = key.read()
  pending[2].resolve(refused)
  await again
  expect(key.known()).toBeNull()
})

it("an answer to an older question does not undo a newer one", async () => {
  const { fake, pending } = gateway()
  const key = watchedVoiceKey(fake)
  const read = key.read()
  const save = key.save("sk-test")
  pending[1].resolve(saved(true))
  await save
  // The read was sent before the save, and answered after it.
  pending[0].resolve(saved(false))
  await read
  expect(key.known()).toBe(true)
})

it("a call refused for want of a key means none is saved", async () => {
  const { fake, pending } = gateway()
  const key = watchedVoiceKey(fake)
  const read = key.read()
  key.missing()
  pending[0].resolve(saved(true))
  await read
  expect(key.known()).toBe(false)
})
