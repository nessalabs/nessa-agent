/**
 * A call to OpenAI's realtime API over WebRTC (`VoiceTransport.webrtc`):
 * the microphone goes up as a track, the voice comes back as one played
 * through an audio element, and realtime events travel on the `oai-events`
 * data channel. The offer is posted to the gateway-named `callUrl` with the
 * short-lived secret; the saved key never reaches the window.
 *
 * Levels are read from an analyser on each stream: the root mean square of
 * its waveform, scaled and capped to 0–1.
 */
import type { VoiceCall } from "@nessa/client"
import {
  TransportRefused,
  type RealtimeConnection,
  type RealtimeEvent,
  type RealtimeTransport,
} from "../application/ports"

/** How long the data channel has to open after the answer. */
const OPEN_TIMEOUT_MS = 15_000

function meter(context: AudioContext, stream: MediaStream): () => number {
  const analyser = context.createAnalyser()
  analyser.fftSize = 512
  context.createMediaStreamSource(stream).connect(analyser)
  const samples = new Float32Array(analyser.fftSize)
  return () => {
    analyser.getFloatTimeDomainData(samples)
    let sum = 0
    for (const sample of samples) sum += sample * sample
    return Math.min(1, Math.sqrt(sum / samples.length) * 4)
  }
}

export function webrtcTransport(): RealtimeTransport {
  return {
    async connect(call: VoiceCall, signal: AbortSignal): Promise<RealtimeConnection> {
      const refused = () => new TransportRefused("Couldn’t connect the call. Try again.")
      let microphone: MediaStream
      try {
        microphone = await navigator.mediaDevices.getUserMedia({
          audio: {
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
          },
        })
      } catch {
        throw new TransportRefused(
          "Nessa can’t use the microphone. Allow it in System Settings › Privacy & Security › Microphone.",
        )
      }
      if (signal.aborted) {
        for (const track of microphone.getTracks()) track.stop()
        throw refused()
      }
      const peer = new RTCPeerConnection()
      const audio = new Audio()
      audio.autoplay = true
      const context = new AudioContext()
      const input = meter(context, microphone)
      let output = () => 0
      let listener: (event: RealtimeEvent) => void = () => {}
      let closedListener: () => void = () => {}
      // One owner of letting go: whichever of an end, a drop or a failed
      // connect comes first releases, and the others find it done.
      let released = false
      const release = () => {
        if (released) return false
        released = true
        signal.removeEventListener("abort", aborted)
        for (const track of microphone.getTracks()) track.stop()
        audio.srcObject = null
        peer.close()
        void context.close().catch(() => {})
        return true
      }
      let opened = false
      const aborted = () => release()
      signal.addEventListener("abort", aborted)
      peer.addEventListener("track", (event) => {
        const [stream] = event.streams
        if (!stream || released) return
        audio.srcObject = stream
        output = meter(context, stream)
      })
      for (const track of microphone.getTracks()) peer.addTrack(track, microphone)
      const channel = peer.createDataChannel("oai-events")
      channel.addEventListener("message", (message) => {
        try {
          const event: unknown = JSON.parse(String(message.data))
          if (
            typeof event === "object" &&
            event !== null &&
            typeof (event as { type?: unknown }).type === "string"
          )
            listener(event as RealtimeEvent)
        } catch {
          console.warn("The voice call sent something unreadable")
        }
      })
      // After it opened, the far side or the network ending the call is a drop.
      const ended = () => {
        if (opened && release()) closedListener()
      }
      channel.addEventListener("close", ended)
      peer.addEventListener("connectionstatechange", () => {
        if (peer.connectionState === "failed" || peer.connectionState === "closed")
          ended()
      })
      try {
        const offer = await peer.createOffer()
        await peer.setLocalDescription(offer)
        const response = await fetch(call.callUrl, {
          method: "POST",
          body: offer.sdp,
          headers: {
            Authorization: `Bearer ${call.clientSecret}`,
            "Content-Type": "application/sdp",
          },
          signal: AbortSignal.any([signal, AbortSignal.timeout(OPEN_TIMEOUT_MS)]),
        })
        if (!response.ok) throw new Error(`call refused (${response.status})`)
        const answer = await response.text()
        if (released) throw new Error("ended while connecting")
        await peer.setRemoteDescription({ type: "answer", sdp: answer })
        await new Promise<void>((resolve, reject) => {
          if (channel.readyState === "open") return resolve()
          const timer = setTimeout(() => reject(new Error("timed out")), OPEN_TIMEOUT_MS)
          const stop = () => {
            clearTimeout(timer)
            reject(new Error("closed while opening"))
          }
          channel.addEventListener("close", stop, { once: true })
          signal.addEventListener("abort", stop, { once: true })
          channel.addEventListener(
            "open",
            () => {
              clearTimeout(timer)
              channel.removeEventListener("close", stop)
              signal.removeEventListener("abort", stop)
              resolve()
            },
            { once: true },
          )
        })
        if (released) throw new Error("ended while connecting")
      } catch {
        release()
        throw refused()
      }
      opened = true
      return {
        send: (event) => {
          if (channel.readyState === "open") channel.send(JSON.stringify(event))
        },
        onEvent: (next) => (listener = next),
        onClosed: (next) => (closedListener = next),
        levels: () =>
          released ? { input: 0, output: 0 } : { input: input(), output: output() },
        setMicrophone: (on) => {
          for (const track of microphone.getAudioTracks()) track.enabled = on
        },
        setSpeaker: (on) => {
          audio.muted = !on
        },
        close: () => void release(),
      }
    },
  }
}
