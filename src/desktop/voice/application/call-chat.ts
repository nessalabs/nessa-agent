/**
 * Keeps the window's chats and its call in step, now that a call goes on
 * while no pane shows its chat. The window is told which chat an open call
 * talks to, so it keeps following that chat off screen and the call hears
 * every reply; and a call whose chat is gone from the window (archived,
 * deleted) is ended, since nothing is left for it to talk to.
 */
import type { VoiceSnapshot } from "./voice-controller"

/** The part of the voice controller this needs. */
export interface ChatCall {
  snapshot(): VoiceSnapshot
  subscribe(listener: () => void): () => void
  end(): void
}

export interface CallChatDependencies {
  readonly call: ChatCall
  /** Says which chat the open call talks to; null when no call is open. */
  readonly hold: (sessionId: string | null) => void
  /** Whether the window still has this chat. */
  readonly has: (sessionId: string) => boolean
  /** Each time the window's chats may have changed. Answers how to stop listening. */
  readonly onChats: (listener: () => void) => () => void
}

/** The chat an open call talks to; null for no call, or one failed or dropped. */
function talkedTo(snapshot: VoiceSnapshot): string | null {
  switch (snapshot.phase) {
    case "connecting":
    case "listening":
    case "working":
    case "speaking":
    case "paused":
      return snapshot.sessionId
    case "idle":
    case "failed":
    case "dropped":
      return null
  }
}

/** Starts keeping them in step; answers how to stop, which lets the chat go. */
export function keepCallChat(dependencies: CallChatDependencies): () => void {
  const { call } = dependencies
  let held: string | null = null
  const sync = () => {
    const wanted = talkedTo(call.snapshot())
    if (wanted !== null && !dependencies.has(wanted)) {
      // Ending tells this again, with no chat.
      call.end()
      return
    }
    if (wanted === held) return
    held = wanted
    dependencies.hold(wanted)
  }
  const stops = [call.subscribe(sync), dependencies.onChats(sync)]
  sync()
  return () => {
    for (const stop of stops) stop()
    if (held !== null) dependencies.hold(null)
  }
}
