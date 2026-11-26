/**
 * The voice key's gateway methods, remembering the last thing the gateway
 * said about the key, so the window can tell before a call starts that it
 * would be refused for want of one, and ask for the key instead of showing
 * a call that is about to fail. The gateway stays the one authority: what
 * is remembered is only its last answer, read again before it is acted on
 * when it says no key is saved (`VoiceProvider`).
 */
import type { VoiceKeyGateway, VoiceKeyOutcome } from "./voice-gateway"

export interface WatchedVoiceKey extends VoiceKeyGateway {
  /**
   * Whether a key is saved, as the gateway last said; null before it has
   * said, or when it would not (Nessa locked, the gateway away).
   */
  known(): boolean | null
  /** A call was refused for want of a key: what was known no longer holds. */
  missing(): void
}

export function watchedVoiceKey(gateway: VoiceKeyGateway): WatchedVoiceKey {
  let known: boolean | null = null
  // Only the answer to the newest question counts: a read sent before a
  // save but answered after it says how things were, not how they are.
  let asked = 0
  const heard = async (
    question: Promise<VoiceKeyOutcome>,
    refusedMeans: "unknown" | "unchanged",
  ): Promise<VoiceKeyOutcome> => {
    const turn = ++asked
    const outcome = await question
    if (turn === asked) {
      if (outcome.kind === "read") known = outcome.keySaved
      else if (refusedMeans === "unknown") known = null
    }
    return outcome
  }
  return {
    read: () => heard(gateway.read(), "unknown"),
    // A save or a removal that was refused changed nothing.
    save: (key) => heard(gateway.save(key), "unchanged"),
    forget: () => heard(gateway.forget(), "unchanged"),
    known: () => known,
    missing: () => {
      asked += 1
      known = false
    },
  }
}
