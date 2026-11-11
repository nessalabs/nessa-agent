/**
 * Keeps the call pill in step with the window's call (`model/call-pill.ts`
 * has the rules). The pill is shown, updated and hidden as the call and the
 * window's place in front change, and a press on it is carried out on the
 * call as the same press in the composer would be.
 */
import {
  callPicture,
  callPressed,
  samePicture,
  type CallPicture,
  type CallPress,
} from "../model/call-pill"
import type { VoiceSnapshot } from "./voice-controller"

/** The pill's window, wherever it is: the app's host, or a browser's other tab. */
export interface CallPillWindow {
  show(picture: CallPicture): Promise<void>
  hide(): Promise<void>
  /** Each press on the pill. Answers how to stop listening. */
  onPress(handler: (press: CallPress) => void): () => void
}

/** Whether this window is the one in front, and each time that changes. */
export interface WindowInFront {
  now(): boolean
  onChange(listener: () => void): () => void
}

/** The part of the voice controller the pill needs. */
export interface PillCall {
  snapshot(): VoiceSnapshot
  subscribe(listener: () => void): () => void
  end(): void
  setMicrophone(on: boolean): void
  setSpeaker(on: boolean): void
}

export interface CallPillDependencies {
  readonly call: PillCall
  readonly pill: CallPillWindow
  readonly inFront: WindowInFront
  /** A chat's title by its session; null when the window does not hold it. */
  readonly title: (sessionId: string) => string | null
  /** Shows a chat in this window: the pill's name was pressed. */
  readonly showChat: (sessionId: string) => void
}

/** Starts following; answers how to stop, which also hides the pill. */
export function followCallWithPill(dependencies: CallPillDependencies): () => void {
  const { call, pill, inFront } = dependencies
  let shown: CallPicture | null = null
  const sync = () => {
    const snapshot = call.snapshot()
    const title =
      snapshot.sessionId === null ? "" : (dependencies.title(snapshot.sessionId) ?? "")
    const wanted = callPicture(snapshot, title, inFront.now())
    if (samePicture(shown, wanted)) return
    shown = wanted
    const sent = wanted === null ? pill.hide() : pill.show(wanted)
    // A pill that cannot be reached costs the pill, never the call.
    sent.catch((error: unknown) =>
      console.warn("Voice: the call pill is out of reach", error),
    )
  }
  const stops = [
    call.subscribe(sync),
    inFront.onChange(sync),
    pill.onPress((press) => {
      const asked = callPressed(press)
      const sessionId = call.snapshot().sessionId
      switch (asked.kind) {
        case "end":
          return call.end()
        case "microphone":
          return call.setMicrophone(asked.on)
        case "speaker":
          return call.setSpeaker(asked.on)
        case "show":
          if (sessionId !== null) dependencies.showChat(sessionId)
      }
    }),
  ]
  sync()
  return () => {
    for (const stop of stops) stop()
    if (shown !== null) {
      shown = null
      pill.hide().catch(() => {})
    }
  }
}
