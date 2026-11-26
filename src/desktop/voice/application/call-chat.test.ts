import { expect, it } from "vitest"
import { keepCallChat } from "./call-chat"
import type { VoiceSnapshot } from "./voice-controller"

const idle: VoiceSnapshot = {
  phase: "idle",
  sessionId: null,
  hearing: "",
  saying: "",
  transcript: [],
  microphoneOn: true,
  speakerOn: true,
  notice: null,
  hint: null,
  fix: null,
}

function scene() {
  let state = idle
  const chats = new Set(["s1", "s2"])
  const callListeners = new Set<() => void>()
  const chatListeners = new Set<() => void>()
  const held: (string | null)[] = []
  let ends = 0
  const set = (next: Partial<VoiceSnapshot>) => {
    state = { ...state, ...next }
    for (const listener of [...callListeners]) listener()
  }
  const stop = keepCallChat({
    call: {
      snapshot: () => state,
      subscribe: (listener) => {
        callListeners.add(listener)
        return () => callListeners.delete(listener)
      },
      end: () => {
        ends += 1
        set(idle)
      },
    },
    hold: (sessionId) => held.push(sessionId),
    has: (sessionId) => chats.has(sessionId),
    onChats: (listener) => {
      chatListeners.add(listener)
      return () => chatListeners.delete(listener)
    },
  })
  return {
    held,
    set,
    stop,
    ends: () => ends,
    phase: () => state.phase,
    remove: (sessionId: string) => {
      chats.delete(sessionId)
      for (const listener of [...chatListeners]) listener()
    },
    listeners: () => callListeners.size + chatListeners.size,
  }
}

it("holds the chat an open call talks to, once, and lets it go when the call ends", () => {
  const s = scene()
  expect(s.held).toEqual([])
  s.set({ phase: "connecting", sessionId: "s1" })
  s.set({ phase: "listening" })
  s.set({ phase: "working", hearing: "hello" })
  expect(s.held).toEqual(["s1"])
  s.set(idle)
  expect(s.held).toEqual(["s1", null])
})

it("lets the chat go when the call fails or drops, though the notice still names it", () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.set({ phase: "dropped", notice: "The call dropped." })
  expect(s.held).toEqual(["s1", null])
  s.set({ phase: "connecting", sessionId: "s2" })
  s.set({ phase: "failed", notice: "No." })
  expect(s.held).toEqual(["s1", null, "s2", null])
  expect(s.ends()).toBe(0)
})

it("ends a call whose chat is gone from the window, and no other", () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.remove("s2")
  expect(s.ends()).toBe(0)
  s.remove("s1")
  expect(s.ends()).toBe(1)
  expect(s.phase()).toBe("idle")
  expect(s.held).toEqual(["s1", null])
})

it("stopping lets the chat go and stops listening", () => {
  const s = scene()
  s.set({ phase: "listening", sessionId: "s1" })
  s.stop()
  expect(s.held).toEqual(["s1", null])
  expect(s.listeners()).toBe(0)
  s.remove("s1")
  expect(s.ends()).toBe(0)
})
