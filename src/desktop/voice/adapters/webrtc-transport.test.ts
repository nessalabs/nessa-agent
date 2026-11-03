import { afterEach, describe, expect, it, vi } from "vitest"
import type { VoiceCall } from "@nessa/client"
import { webrtcTransport } from "./webrtc-transport"

const call = {
  transport: "webrtc",
  callUrl: "https://example.test/call",
  clientSecret: "ek_1",
  expiresAt: 0,
  model: "m",
} as unknown as VoiceCall

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe("webrtcTransport", () => {
  it("lets go of the microphone when setting up the call throws", async () => {
    const stop = vi.fn()
    const closed = vi.fn()
    vi.stubGlobal("navigator", {
      mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop }] }) },
    })
    vi.stubGlobal(
      "Audio",
      class {
        autoplay = false
        srcObject: unknown = null
      },
    )
    vi.stubGlobal(
      "RTCPeerConnection",
      class {
        close = closed
      },
    )
    vi.stubGlobal(
      "AudioContext",
      class {
        constructor() {
          throw new Error("no audio device")
        }
      },
    )
    await expect(
      webrtcTransport().connect(call, new AbortController().signal),
    ).rejects.toThrow("Couldn’t set up the call’s audio")
    expect(stop).toHaveBeenCalledOnce()
    expect(closed).toHaveBeenCalledOnce()
  })

  it("gives up on a microphone macOS never answers for, and stops it if it comes later", async () => {
    vi.useFakeTimers()
    try {
      const stop = vi.fn()
      let grant: (stream: unknown) => void = () => {}
      vi.stubGlobal("navigator", {
        mediaDevices: {
          getUserMedia: () => new Promise((resolve) => (grant = resolve)),
        },
      })
      vi.stubGlobal("Audio", class {})
      const connecting = webrtcTransport().connect(call, new AbortController().signal)
      const outcome = expect(connecting).rejects.toThrow(
        "macOS didn’t answer for the microphone",
      )
      await vi.advanceTimersByTimeAsync(20_000)
      await outcome
      grant({ getTracks: () => [{ stop }] })
      await vi.advanceTimersByTimeAsync(0)
      expect(stop).toHaveBeenCalledOnce()
    } finally {
      vi.useRealTimers()
    }
  })

  it.each([
    [
      "the network fails",
      () => Promise.reject(new TypeError("offline")),
      "Couldn’t reach OpenAI",
    ],
    [
      "OpenAI refuses",
      async () => new Response("no", { status: 400 }),
      "OpenAI didn’t accept the call",
    ],
  ])("says where the call failed when %s", async (_, answer, sentence) => {
    const stop = vi.fn()
    vi.stubGlobal("navigator", {
      mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop }] }) },
    })
    vi.stubGlobal("Audio", class {})
    vi.stubGlobal(
      "RTCPeerConnection",
      class {
        addEventListener() {}
        addTrack() {}
        createDataChannel() {
          return { addEventListener() {}, readyState: "connecting" }
        }
        createOffer = async () => ({ type: "offer", sdp: "v=0" })
        setLocalDescription = async () => {}
        close() {}
      },
    )
    vi.stubGlobal(
      "AudioContext",
      class {
        createAnalyser = () => ({ fftSize: 0, getFloatTimeDomainData() {} })
        createMediaStreamSource = () => ({ connect() {} })
        resume = async () => {}
        close = async () => {}
      },
    )
    vi.stubGlobal("fetch", answer)
    vi.spyOn(console, "warn").mockImplementation(() => {})
    await expect(
      webrtcTransport().connect(call, new AbortController().signal),
    ).rejects.toThrow(sentence)
    expect(stop).toHaveBeenCalledOnce()
  })
})
