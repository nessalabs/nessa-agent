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
  let title: string | null = "Fix the build"
  const callListeners = new Set<() => void>()
  const titleListeners = new Set<() => void>()
  let press: (press: CallPress) => void = () => {}
  let refuse = 0
  const told: (CallPicture | null)[] = []
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
      set: async (picture) => {
        // A slow host: the first telling takes longer than the next, so
        // only an ordered sender keeps them in order.
        await new Promise((done) => setTimeout(done, told.length === 0 ? 5 : 0))
        if (refuse > 0) {
          refuse -= 1
          throw new Error("no pill")
        }
        told.push(picture)
      },
      onPress: (handler) => {
        press = handler
        return () => (press = () => {})
      },
    },
    title: (sessionId) => (sessionId === "s1" ? title : null),
    onTitles: (listener) => {
      titleListeners.add(listener)
      return () => titleListeners.delete(listener)
    },
    showChat: (sessionId) => did.push(`show ${sessionId}`),
  })
  return {
    told,
    did,
    set,
    stop,
    press: (name: CallPress) => press(name),
    rename: (next: string | null) => {
      title = next
      for (const listener of titleListeners) listener()
    },
    refuseNext: () => (refuse += 1),
    settled: () => new Promise((done) => setTimeout(done, 30)),
    listeners: () => callListeners.size + titleListeners.size,
  }
}

it("tells the host what an open call looks like, in the order it changed, and null when it ends", async () => {
  const s = scene()
  await s.settled()
  expect(s.told).toEqual([])
  s.set({ phase: "connecting", sessionId: "s1" })
  s.set({ hearing: "hello" })
  s.set({ phase: "listening" })
  s.set({ phase: "dropped" })
  await s.settled()
  expect(s.told).toEqual([
    { phase: "connecting", title: "Fix the build", microphoneOn: true, speakerOn: true },
    { phase: "listening", title: "Fix the build", microphoneOn: true, speakerOn: true },
    null,
  ])
})

it("follows a title that changes while the call is open", async () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.rename("Ship the pill")
  s.rename("Ship the pill")
  await s.settled()
  expect(s.told.map((picture) => picture?.title)).toEqual([
    "Fix the build",
    "Ship the pill",
  ])
})

it("tells the host again after a telling failed, even when the picture is the same", async () => {
  const s = scene()
  s.refuseNext()
  s.set({ phase: "listening", sessionId: "s1" })
  await s.settled()
  expect(s.told).toEqual([])
  s.set({ hearing: "still here" })
  await s.settled()
  expect(s.told).toHaveLength(1)
})

it("carries each press out on the call, and the pill follows what the call then says", async () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.press("mute")
  await s.settled()
  expect(s.told.at(-1)).toMatchObject({ microphoneOn: false })
  s.press("unmute")
  s.press("speaker-off")
  await s.settled()
  expect(s.told.at(-1)).toMatchObject({ microphoneOn: true, speakerOn: false })
  s.press("speaker-on")
  s.press("open")
  s.press("end")
  await s.settled()
  expect(s.did).toEqual([
    "microphone false",
    "microphone true",
    "speaker false",
    "speaker true",
    "show s1",
    "end",
  ])
  expect(s.told.at(-1)).toBeNull()
})

it("stopping clears the pill and lets go of everything it listened to", async () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.stop()
  await s.settled()
  expect(s.told.at(-1)).toBeNull()
  expect(s.listeners()).toBe(0)
  s.press("end")
  expect(s.did).toEqual([])
})
