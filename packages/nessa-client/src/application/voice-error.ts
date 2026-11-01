import { NessaRpcError } from "./rpc-error.js"
import { VoiceErrorCode } from "../generated/product.js"

/**
 * A code the `voice.*` methods answer with, including the session codes the
 * route uses beside {@link VoiceErrorCode}. Membership is the only way a raw
 * code becomes one of these.
 */
export const VoiceRefusalCode = {
  ...VoiceErrorCode,
  Forbidden: "forbidden",
  Unauthorized: "unauthorized",
  InvalidRequest: "invalid_request",
  TemporarilyUnavailable: "temporarily_unavailable",
  /** `voice.startCall`: the gateway keeps no conversations. */
  ConversationsNotConfigured: "conversations_not_configured",
  /** `voice.startCall`: no such conversation, or not one the caller may use. */
  ConversationNotFound: "conversation_not_found",
} as const

export type VoiceRefusalCode = (typeof VoiceRefusalCode)[keyof typeof VoiceRefusalCode]

const voiceCodes = new Set<string>(Object.values(VoiceRefusalCode))

export function voiceRefusalCode(code: unknown): VoiceRefusalCode | undefined {
  return typeof code === "string" && voiceCodes.has(code)
    ? (code as VoiceRefusalCode)
    : undefined
}

/** The method a {@link NessaVoiceError} answers. */
export type VoiceMethod =
  "voice.status" | "voice.saveKey" | "voice.forgetKey" | "voice.startCall"

/**
 * A `voice.*` method that did not answer with a result.
 *
 * `refusal` is a code this build knows, or `undefined` when there was no
 * answer, the answer was not one the schema describes, or the code is not
 * one of {@link VoiceRefusalCode}. Nothing is retried for you.
 */
export class NessaVoiceError extends Error {
  readonly refusal: VoiceRefusalCode | undefined

  constructor(
    readonly method: VoiceMethod,
    cause: unknown,
  ) {
    const refusal =
      cause instanceof NessaRpcError ? voiceRefusalCode(cause.code) : undefined
    super(refusal ? `${method} was refused (${refusal})` : `${method} failed`, { cause })
    this.refusal = refusal
    this.name = "NessaVoiceError"
  }
}
