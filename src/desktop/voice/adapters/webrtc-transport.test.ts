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

afterEach(() => vi.unstubAllGlobals())

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
    ).rejects.toThrow("Couldn’t connect the call")
    expect(stop).toHaveBeenCalledOnce()
    expect(closed).toHaveBeenCalledOnce()
  })
})
