/**
 * The gateway's voice methods (`client.voice`) in the window's words: a call
 * started for the controller, and the key for Settings. Each refusal is a
 * sentence from a total table, so a code the protocol adds is a type error
 * here rather than a sentence nobody wrote.
 */
import {
  NessaVoiceError,
  VoiceRefusalCode,
  type VoiceApi,
  type VoiceCall,
  type VoiceRefusalCode as VoiceRefusal,
} from "@nessa/client"
import { VoiceStartRefused, type VoiceGateway } from "../application/ports"

/** What the voice adapters ask of the window's gateway client. */
export interface VoiceClient {
  readonly voice: VoiceApi
}

const refusals: Record<VoiceRefusal, string> = {
  [VoiceRefusalCode.VoiceNotConfigured]: "Voice isn’t available on this gateway.",
  [VoiceRefusalCode.VoiceKeyMissing]:
    "Add your OpenAI key in Settings › Connections › Voice to talk to your agents.",
  [VoiceRefusalCode.VoiceKeyRejected]:
    "OpenAI refused the saved key. Check it in Settings › Connections › Voice.",
  [VoiceRefusalCode.VoiceUnreachable]:
    "Couldn’t reach OpenAI. Check the connection and try again.",
  [VoiceRefusalCode.VoiceStorageUnavailable]:
    "Couldn’t read or change the saved key. Try again.",
  [VoiceRefusalCode.VoiceAuditUnavailable]:
    "Couldn’t keep a record of this, so nothing changed. Try again.",
  [VoiceRefusalCode.VaultLocked]: "Unlock Nessa to use voice.",
  [VoiceRefusalCode.Forbidden]: "This window isn’t allowed to use voice.",
  [VoiceRefusalCode.Unauthorized]: "This window isn’t allowed to use voice.",
  [VoiceRefusalCode.InvalidRequest]: "The gateway didn’t understand the request.",
  [VoiceRefusalCode.TemporarilyUnavailable]: "The gateway is busy. Try again.",
  [VoiceRefusalCode.ConversationsNotConfigured]:
    "This gateway doesn’t keep chats to talk to.",
  [VoiceRefusalCode.ConversationNotFound]: "This chat isn’t there anymore.",
}

const unanswered = "The gateway didn’t answer. Try again."

/** The sentence for a voice method that did not answer with a result. */
export function voiceRefusalSentence(error: unknown): string {
  return error instanceof NessaVoiceError && error.refusal
    ? refusals[error.refusal]
    : unanswered
}

/** The gateway side of a call, over the window's client. */
export function voiceGateway(connected: () => Promise<VoiceClient>): VoiceGateway {
  return {
    async startCall(conversationId: string): Promise<VoiceCall> {
      try {
        return await (await connected()).voice.startCall(conversationId)
      } catch (error) {
        throw new VoiceStartRefused(voiceRefusalSentence(error))
      }
    },
  }
}

/** Whether a key is saved, or why the gateway would not say. */
export type VoiceKeyOutcome =
  | { readonly kind: "read"; readonly keySaved: boolean }
  | { readonly kind: "refused"; readonly sentence: string }

/** The key, as Settings manages it. */
export interface VoiceKeyGateway {
  read(): Promise<VoiceKeyOutcome>
  save(key: string): Promise<VoiceKeyOutcome>
  forget(): Promise<VoiceKeyOutcome>
}

export function voiceKeyGateway(connected: () => Promise<VoiceClient>): VoiceKeyGateway {
  const ask = async (
    run: (voice: VoiceApi) => Promise<{ keySaved: boolean }>,
  ): Promise<VoiceKeyOutcome> => {
    try {
      return { kind: "read", keySaved: (await run((await connected()).voice)).keySaved }
    } catch (error) {
      return { kind: "refused", sentence: voiceRefusalSentence(error) }
    }
  }
  return {
    read: () => ask((voice) => voice.status()),
    save: (key) => ask((voice) => voice.saveKey(key)),
    forget: () => ask((voice) => voice.forgetKey()),
  }
}
