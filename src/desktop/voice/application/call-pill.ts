/**
 * Keeps the call pill in step with the window's call (`model/call-pill.ts`
 * has the rules). The pill's host is told what the call looks like at every
 * change, and a press on the pill is carried out on the call as the same
 * press in the composer would be. The host is also told whether a pane
 * shows the call's chat. Whether the pill is on screen is the host's to
 * decide, from that and from which window is in front.
 */
import {
  callPicture,
  callPressed,
  samePicture,
  type CallPicture,
  type CallPress,
} from "../model/call-pill"
import type { VoiceSnapshot } from "./voice-controller"

/** The pill's host. */
export interface CallPillHost {
  /**
   * What the call looks like now, null when no call is open, and whether a
   * pane shows the chat it talks to.
   */
  set(picture: CallPicture | null, chatShown: boolean): Promise<void>
  /** Each press on the pill. Answers how to stop listening. */
  onPress(handler: (press: CallPress) => void): () => void
}

/** The part of the voice controller the pill needs. */
export interface PillCall {
  snapshot(): VoiceSnapshot
  /** Whether a pane shows the chat the call talks to. */
  chatShown(): boolean
  subscribe(listener: () => void): () => void
  end(): void
  setMicrophone(on: boolean): void
  setSpeaker(on: boolean): void
}

export interface CallPillDependencies {
  readonly call: PillCall
  readonly pill: CallPillHost
  /** A chat's title by its session; null when the window does not hold it. */
  readonly title: (sessionId: string) => string | null
  /** Each time a title may have changed. Answers how to stop listening. */
  readonly onTitles: (listener: () => void) => () => void
  /** Shows a chat in this window: the pill's name was pressed. */
  readonly showChat: (sessionId: string) => void
}

/** What the host is told: the picture, and whether a pane shows its chat. */
interface Told {
  readonly picture: CallPicture | null
  readonly chatShown: boolean
}

/** Starts following; answers how to stop, which also clears the pill. */
export function followCallWithPill(dependencies: CallPillDependencies): () => void {
  const { call, pill } = dependencies
  // What the host was last told, or `unknown` when a telling failed: the
  // next change then tells it again, whatever the picture.
  let told: Told | "unknown" = { picture: null, chatShown: false }
  // One telling at a time, in order: two sent at once could land reversed,
  // and the host would keep the older picture.
  let sending: Promise<void> = Promise.resolve()
  const tell = (wanted: Told) => {
    told = wanted
    sending = sending
      .then(() => pill.set(wanted.picture, wanted.chatShown))
      .catch((error: unknown) => {
        // A pill that cannot be reached costs the pill, never the call.
        console.warn("Voice: the call pill is out of reach", error)
        if (told === wanted) told = "unknown"
      })
  }
  const sync = () => {
    const snapshot = call.snapshot()
    const title =
      snapshot.sessionId === null ? "" : (dependencies.title(snapshot.sessionId) ?? "")
    const picture = callPicture(snapshot, title)
    // With no call there is no chat to show, whatever the panes hold.
    const wanted: Told = { picture, chatShown: picture !== null && call.chatShown() }
    if (
      told !== "unknown" &&
      samePicture(told.picture, wanted.picture) &&
      told.chatShown === wanted.chatShown
    )
      return
    tell(wanted)
  }
  const stops = [
    call.subscribe(sync),
    dependencies.onTitles(sync),
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
    if (told === "unknown" || told.picture !== null)
      tell({ picture: null, chatShown: false })
  }
}
