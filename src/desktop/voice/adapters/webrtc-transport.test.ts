import { afterEach, describe, expect, it, vi } from "vitest"
import type { VoiceCall } from "@nessa/client"
import { audioBytesIn, tracksLive, webrtcTransport } from "./webrtc-transport"

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

  it("checks whether audio is going out and the microphone gives sound", async () => {
    vi.useFakeTimers()
    try {
      let bytes = 100
      let level = 0.5
      let statsFail = false
      const channel = {
        readyState: "connecting",
        listeners: new Map<string, () => void>(),
        addEventListener(type: string, run: () => void) {
          this.listeners.set(type, run)
        },
        removeEventListener() {},
      }
      vi.stubGlobal("navigator", {
        mediaDevices: {
          getUserMedia: async () => ({
            getTracks: () => [{ stop() {} }],
            getAudioTracks: () => [{ readyState: "live", muted: false }],
          }),
        },
      })
      vi.stubGlobal("Audio", class {})
      vi.stubGlobal(
        "RTCPeerConnection",
        class {
          addEventListener() {}
          addTrack() {}
          createDataChannel() {
            return channel
          }
          createOffer = async () => ({ type: "offer", sdp: "v=0" })
          setLocalDescription = async () => {}
          setRemoteDescription = async () => {
            channel.readyState = "open"
          }
          getStats = async () => {
            if (statsFail) throw new Error("no stats")
            return new Map([
              ["a", { type: "outbound-rtp", kind: "audio", bytesSent: bytes }],
            ])
          }
          close() {}
        },
      )
      vi.stubGlobal(
        "AudioContext",
        class {
          state = "running"
          createAnalyser = () => ({
            fftSize: 4,
            getFloatTimeDomainData(samples: Float32Array) {
              samples.fill(level)
            },
          })
          createMediaStreamSource = () => ({ connect() {} })
          resume = async () => {}
          close = async () => {}
        },
      )
      vi.stubGlobal("fetch", async () => new Response("v=0", { status: 201 }))
      const connection = await webrtcTransport().connect(
        call,
        new AbortController().signal,
      )
      const sending = connection.check()
      bytes = 200
      await vi.advanceTimersByTimeAsync(1_100)
      expect(await sending).toEqual({
        answered: false,
        microphoneLive: true,
        audioSent: true,
        microphoneSound: true,
      })
      level = 0
      const stalled = connection.check()
      await vi.advanceTimersByTimeAsync(1_100)
      expect(await stalled).toEqual({
        answered: false,
        microphoneLive: true,
        audioSent: false,
        microphoneSound: false,
      })
      statsFail = true
      const unread = connection.check()
      await vi.advanceTimersByTimeAsync(1_100)
      expect((await unread).audioSent).toBeNull()
      connection.close()
      expect(await connection.check()).toEqual({
        answered: false,
        microphoneLive: false,
        audioSent: false,
        microphoneSound: false,
      })
    } finally {
      vi.useRealTimers()
    }
  })
})

describe("the call's own audio statistics", () => {
  it("counts outgoing audio bytes under either name for its kind", () => {
    expect(
      audioBytesIn([
        { type: "outbound-rtp", kind: "audio", bytesSent: 100 },
        { type: "outbound-rtp", mediaType: "audio", bytesSent: 20 },
        { type: "outbound-rtp", bytesSent: 3 },
        { type: "outbound-rtp", kind: "video", bytesSent: 999 },
        { type: "inbound-rtp", kind: "audio", bytesSent: 999 },
      ]),
    ).toBe(123)
  })

  it("says nothing when no report counts outgoing audio", () => {
    expect(audioBytesIn([])).toBeNull()
    expect(audioBytesIn([{ type: "transport", bytesSent: 5 }])).toBeNull()
    expect(audioBytesIn([{ type: "outbound-rtp", kind: "audio" }])).toBeNull()
  })

  it("calls a microphone live only while every track is live and unmuted", () => {
    expect(tracksLive([])).toBeNull()
    expect(tracksLive([{ readyState: "live", muted: false }])).toBe(true)
    expect(tracksLive([{ readyState: "live", muted: true }])).toBe(false)
    expect(tracksLive([{ readyState: "ended", muted: false }])).toBe(false)
  })
})
