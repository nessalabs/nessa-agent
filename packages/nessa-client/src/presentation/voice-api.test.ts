import { describe, expect, it, vi } from "vitest"

import { NessaRpcError } from "../application/rpc-error.js"
import { NessaVoiceError, VoiceRefusalCode } from "../application/voice-error.js"
import { createVoiceApi } from "./voice-api.js"

function session(answer: (method: string, params: unknown) => unknown) {
  const calls: { method: string; params: unknown }[] = []
  const request = vi.fn(async (method: string, params: unknown) => {
    calls.push({ method, params })
    return answer(method, params)
  })
  return { api: createVoiceApi({ request }), calls }
}

const call = {
  transport: "webrtc",
  callUrl: "https://api.openai.com/v1/realtime/calls",
  clientSecret: "ek_1",
  expiresAt: 60_000,
  model: "gpt-realtime",
}

describe("client.voice", () => {
  it("asks each method with its params and reads its answer", async () => {
    const { api, calls } = session((method) =>
      method === "voice.startCall"
        ? call
        : { keySaved: method === "voice.saveKey", model: "gpt-realtime" },
    )
    expect(await api.status()).toEqual({ keySaved: false, model: "gpt-realtime" })
    expect((await api.saveKey("sk-1")).keySaved).toBe(true)
    expect((await api.forgetKey()).keySaved).toBe(false)
    expect(await api.startCall("c-1")).toEqual(call)
    expect(calls).toEqual([
      { method: "voice.status", params: {} },
      { method: "voice.saveKey", params: { key: "sk-1" } },
      { method: "voice.forgetKey", params: {} },
      { method: "voice.startCall", params: { conversationId: "c-1" } },
    ])
  })

  it("refuses an answer outside the schema", async () => {
    for (const answer of [
      { ...call, transport: "carrier-pigeon" },
      { ...call, clientSecret: "" },
      { ...call, expiresAt: -1 },
      { ...call, key: "sk-1" },
    ]) {
      const { api } = session(() => answer)
      const error = await api.startCall("c-1").catch((caught: unknown) => caught)
      expect(error).toBeInstanceOf(NessaVoiceError)
      expect((error as NessaVoiceError).refusal).toBeUndefined()
    }
  })

  it("names a refusal this build knows", async () => {
    const { api } = session(() => {
      throw new NessaRpcError("voice_key_missing", "refused")
    })
    const error = (await api
      .startCall("c-1")
      .catch((caught: unknown) => caught)) as NessaVoiceError
    expect(error.refusal).toBe(VoiceRefusalCode.VoiceKeyMissing)
    expect(error.method).toBe("voice.startCall")
  })

  it("names the conversation refusals a call can meet", async () => {
    for (const code of [
      VoiceRefusalCode.ConversationNotFound,
      VoiceRefusalCode.ConversationsNotConfigured,
    ]) {
      const { api } = session(() => {
        throw new NessaRpcError(code, "refused")
      })
      const error = await api.startCall("c-1").catch((caught: unknown) => caught)
      expect((error as NessaVoiceError).refusal).toBe(code)
    }
  })
})
