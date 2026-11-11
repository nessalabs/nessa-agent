import { expect, it } from "vitest"
import type { CallPicture, CallPress } from "../model/call-pill"
import { followCallWithPill } from "./call-pill"
import type { VoiceSnapshot } from "./voice-controller"

const idle: VoiceSnapshot = {
  phase: "idle",
  sessionId: null,
  hearing: "",
  saying: "",
  microphoneOn: true,
  speakerOn: true,
  notice: null,
  hint: null,
  fix: null,
}

function scene() {
  let state = idle
  let front = true
  const callListeners = new Set<() => void>()
  const frontListeners = new Set<() => void>()
  let press: (press: CallPress) => void = () => {}
  const shown: (CallPicture | null)[] = []
  const did: string[] = []
  const set = (next: Partial<VoiceSnapshot>) => {
    state = { ...state, ...next }
    for (const listener of callListeners) listener()
  }
  const stop = followCallWithPill({
    call: {
      snapshot: () => state,
      subscribe: (listener) => {
        callListeners.add(listener)
        return () => callListeners.delete(listener)
      },
      end: () => {
        did.push("end")
        set(idle)
      },
      setMicrophone: (on) => {
        did.push(`microphone ${on}`)
        set({ microphoneOn: on })
      },
      setSpeaker: (on) => {
        did.push(`speaker ${on}`)
        set({ speakerOn: on })
      },
    },
    pill: {
      show: async (picture) => void shown.push(picture),
      hide: async () => void shown.push(null),
      onPress: (handler) => {
        press = handler
        return () => (press = () => {})
      },
    },
    inFront: {
      now: () => front,
      onChange: (listener) => {
        frontListeners.add(listener)
        return () => frontListeners.delete(listener)
      },
    },
    title: (sessionId) => (sessionId === "s1" ? "Fix the build" : null),
    showChat: (sessionId) => did.push(`show ${sessionId}`),
  })
  return {
    shown,
    did,
    set,
    stop,
    press: (name: CallPress) => press(name),
    front: (now: boolean) => {
      front = now
      for (const listener of frontListeners) listener()
    },
    listeners: () => callListeners.size + frontListeners.size,
  }
}

it("shows the pill when the window with an open call leaves the front, and hides it on return", () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  expect(s.shown).toEqual([])
  s.front(false)
  expect(s.shown).toEqual([
    { phase: "listening", title: "Fix the build", microphoneOn: true, speakerOn: true },
  ])
  s.front(true)
  expect(s.shown.at(-1)).toBeNull()
  expect(s.shown).toHaveLength(2)
})

it("shows nothing for a window away with no call, and updates the pill only when it would draw differently", () => {
  const s = scene()
  s.front(false)
  expect(s.shown).toEqual([])
  s.set({ phase: "connecting", sessionId: "s1" })
  s.set({ hearing: "hello" })
  s.set({ phase: "listening" })
  expect(s.shown.map((picture) => picture?.phase)).toEqual(["connecting", "listening"])
})

it("hides the pill when the call ends or drops", () => {
  const s = scene()
  s.front(false)
  s.set({ phase: "speaking", sessionId: "s1" })
  s.set({ phase: "dropped" })
  expect(s.shown.at(-1)).toBeNull()
})

it("carries each press out on the call, and the pill follows what the call then says", () => {
  const s = scene()
  s.front(false)
  s.set({ phase: "listening", sessionId: "s1" })
  s.press("mute")
  expect(s.shown.at(-1)).toMatchObject({ microphoneOn: false })
  s.press("unmute")
  s.press("speaker-off")
  expect(s.shown.at(-1)).toMatchObject({ microphoneOn: true, speakerOn: false })
  s.press("speaker-on")
  s.press("open")
  s.press("end")
  expect(s.did).toEqual([
    "microphone false",
    "microphone true",
    "speaker false",
    "speaker true",
    "show s1",
    "end",
  ])
  expect(s.shown.at(-1)).toBeNull()
})

it("stopping hides a pill that shows and lets go of everything it listened to", () => {
  const s = scene()
  s.front(false)
  s.set({ phase: "listening", sessionId: "s1" })
  s.stop()
  expect(s.shown.at(-1)).toBeNull()
  expect(s.listeners()).toBe(0)
  s.press("end")
  expect(s.did).toEqual([])
})
