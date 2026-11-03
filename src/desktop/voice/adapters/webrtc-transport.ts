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

/** How long `check` watches the call's audio for. */
const CHECK_MS = 1_000

/** A level the microphone counts as sound at, above a quiet room's hiss. */
const SOUND_LEVEL = 0.02

/** Audio bytes the call has sent so far, from the peer's own statistics. */
async function audioBytesSent(peer: RTCPeerConnection): Promise<number> {
  let sent = 0
  for (const report of (await peer.getStats()).values()) {
    const stat = report as { type?: string; kind?: string; bytesSent?: number }
    if (stat.type === "outbound-rtp" && stat.kind === "audio") sent += stat.bytesSent ?? 0
  }
  return sent
}

/** How long the data channel has to open after the answer. */
const OPEN_TIMEOUT_MS = 15_000

/**
 * How long macOS has to answer for the microphone. WebKit's request has no
 * limit of its own: unanswered, a call would say "Connecting…" forever.
 */
const MICROPHONE_TIMEOUT_MS = 20_000

/** Where a call that failed to connect got to, each with what to tell the person. */
type Stage = "setup" | "offer" | "answer" | "open"

const failed: Record<Stage, string> = {
  setup: "Couldn’t set up the call’s audio on this Mac. Try again.",
  offer: "Couldn’t reach OpenAI to connect the call. Check the connection and try again.",
  answer: "OpenAI didn’t accept the call. Try again.",
  open: "OpenAI accepted the call but never opened it. Try again.",
}

const microphoneRefused =
  "Nessa can’t use the microphone. Allow it in System Settings › Privacy & Security › Microphone."

const microphoneUnanswered =
  "macOS didn’t answer for the microphone. Check System Settings › Privacy & Security › Microphone, then try again."

/**
 * The microphone, asked for once with a time limit. A stream that arrives
 * after the limit, or after the call ended, is stopped as it arrives.
 */
function askMicrophone(signal: AbortSignal): Promise<MediaStream> {
  return new Promise<MediaStream>((resolve, reject) => {
    let late = false
    const giveUp = (sentence: string) => {
      late = true
      clearTimeout(timer)
      signal.removeEventListener("abort", aborted)
      reject(new TransportRefused(sentence))
    }
    const aborted = () => giveUp("Couldn’t connect the call. Try again.")
    // A call ended before it asked never asks.
    if (signal.aborted)
      return reject(new TransportRefused("Couldn’t connect the call. Try again."))
    const timer = setTimeout(() => giveUp(microphoneUnanswered), MICROPHONE_TIMEOUT_MS)
    signal.addEventListener("abort", aborted, { once: true })
    let asked: Promise<MediaStream>
    try {
      asked = navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      })
    } catch (error) {
      // No microphone at all in this web view.
      console.warn("Voice: the microphone can’t be asked for", error)
      return giveUp(microphoneRefused)
    }
    asked.then(
      (stream) => {
        if (late) {
          for (const track of stream.getTracks()) track.stop()
          return
        }
        clearTimeout(timer)
        signal.removeEventListener("abort", aborted)
        resolve(stream)
      },
      (error: unknown) => {
        if (late) return
        console.warn("Voice: the microphone was refused", error)
        giveUp(microphoneRefused)
      },
    )
  })
}

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
      const audio = new Audio()
      audio.autoplay = true
      let microphone: MediaStream
      try {
        microphone = await askMicrophone(signal)
      } catch (error) {
        throw error instanceof TransportRefused
          ? error
          : new TransportRefused(microphoneRefused)
      }
      // Everything below is let go of by `release`, which owns the microphone
      // from here on: a constructor that throws still stops it.
      let peer: RTCPeerConnection | null = null
      let context: AudioContext | null = null
      let input = () => 0
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
        peer?.close()
        void context?.close().catch(() => {})
        return true
      }
      let opened = false
      let answered = false
      const aborted = () => release()
      signal.addEventListener("abort", aborted)
      let channel: RTCDataChannel
      let stage: Stage = "setup"
      try {
        const connection = new RTCPeerConnection()
        peer = connection
        const meters = new AudioContext()
        context = meters
        // A context made away from a click starts suspended, and its meters
        // then read silence. A call is reason enough to run it.
        void meters.resume().catch(() => {})
        input = meter(meters, microphone)
        connection.addEventListener("track", (event) => {
          const [stream] = event.streams
          if (!stream || released) return
          audio.srcObject = stream
          output = meter(meters, stream)
        })
        for (const track of microphone.getTracks()) connection.addTrack(track, microphone)
        channel = connection.createDataChannel("oai-events")
        channel.addEventListener("message", (message) => {
          answered = true
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
        connection.addEventListener("connectionstatechange", () => {
          if (
            connection.connectionState === "failed" ||
            connection.connectionState === "closed"
          )
            ended()
        })
        const offer = await connection.createOffer()
        await connection.setLocalDescription(offer)
        stage = "offer"
        const response = await fetch(call.callUrl, {
          method: "POST",
          body: offer.sdp,
          headers: {
            Authorization: `Bearer ${call.clientSecret}`,
            "Content-Type": "application/sdp",
          },
          signal: AbortSignal.any([signal, AbortSignal.timeout(OPEN_TIMEOUT_MS)]),
        })
        stage = "answer"
        if (!response.ok) throw new Error(`call refused (${response.status})`)
        const answer = await response.text()
        if (released) throw new Error("ended while connecting")
        await connection.setRemoteDescription({ type: "answer", sdp: answer })
        stage = "open"
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
      } catch (error) {
        // An ended call is not a failure to tell anyone about.
        const ended = signal.aborted
        release()
        if (!ended) console.warn(`Voice: the call failed at ${stage}`, error)
        throw new TransportRefused(
          ended ? "Couldn’t connect the call. Try again." : failed[stage],
        )
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
        check: async () => {
          const live = peer
          if (released || !live)
            return { answered, audioSent: false, microphoneSound: false }
          // A statistics read that fails says nothing either way.
          const before = await audioBytesSent(live).catch(() => null)
          let loudest = 0
          const until = Date.now() + CHECK_MS
          while (Date.now() < until) {
            loudest = Math.max(loudest, input())
            await new Promise((resolve) => setTimeout(resolve, 50))
          }
          const after = await audioBytesSent(live).catch(() => null)
          return {
            answered,
            audioSent: before === null || after === null ? null : after > before,
            microphoneSound: context?.state === "running" ? loudest >= SOUND_LEVEL : null,
          }
        },
        close: () => void release(),
      }
    },
  }
}
