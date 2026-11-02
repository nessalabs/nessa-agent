import { NessaVoiceError, type VoiceMethod } from "../application/voice-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod, type VoiceCall, type VoiceStatus } from "../generated/product.js"
import { voiceCall, voiceStatus } from "../protocol/voice-validate.js"

/**
 * The gateway's realtime voice key, and the calls it starts with it. The key
 * is saved in the gateway's vault and never comes back: `status` says only
 * whether one is saved. `startCall` answers how to connect one short-lived
 * call about a conversation; what the call is told and may call was fixed
 * by the gateway when it minted it. Nothing is retried for you.
 */
export type VoiceApi = {
  status(): Promise<VoiceStatus>
  saveKey(key: string): Promise<VoiceStatus>
  forgetKey(): Promise<VoiceStatus>
  startCall(conversationId: string): Promise<VoiceCall>
}

export function createVoiceApi(session: RpcRequester): VoiceApi {
  async function call<T>(
    method: VoiceMethod,
    params: Record<string, unknown>,
    read: (value: unknown) => T,
  ): Promise<T> {
    try {
      return read(await session.request(method, params))
    } catch (cause) {
      throw new NessaVoiceError(method, cause)
    }
  }
  return {
    status: () => call(ProductMethod.VoiceStatus, {}, voiceStatus),
    saveKey: (key) => call(ProductMethod.VoiceSaveKey, { key }, voiceStatus),
    forgetKey: () => call(ProductMethod.VoiceForgetKey, {}, voiceStatus),
    startCall: (conversationId) =>
      call(ProductMethod.VoiceStartCall, { conversationId }, voiceCall),
  }
}
