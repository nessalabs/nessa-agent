/**
 * A call to a scripted provider (`VoiceTransport.scripted`): the same
 * realtime events as OpenAI's, over a WebSocket, with no audio. The gateway
 * names it only when it was started with `NESSA_VOICE_REALTIME` pointing at
 * one, which is how the conversation stress check talks to the window.
 */
import type { VoiceCall } from "@nessa/client"
import {
  TransportRefused,
  type RealtimeConnection,
  type RealtimeEvent,
  type RealtimeTransport,
} from "../application/ports"

export function scriptedTransport(
  open: (url: string) => WebSocket = (url) => new WebSocket(url),
): RealtimeTransport {
  return {
    connect(call: VoiceCall, signal: AbortSignal): Promise<RealtimeConnection> {
      return new Promise((resolve, reject) => {
        if (signal.aborted)
          return reject(new TransportRefused("Couldn’t connect the call."))
        const url = new URL(call.callUrl)
        url.searchParams.set("secret", call.clientSecret)
        const socket = open(url.toString())
        let listener: (event: RealtimeEvent) => void = () => {}
        let closedListener: () => void = () => {}
        let closing = false
        let opened = false
        socket.addEventListener("message", (message) => {
          try {
            const event: unknown = JSON.parse(String(message.data))
            if (
              typeof event === "object" &&
              event !== null &&
              typeof (event as { type?: unknown }).type === "string"
            )
              listener(event as RealtimeEvent)
          } catch {
            console.warn("The scripted voice sent something unreadable")
          }
        })
        socket.addEventListener("close", () => {
          if (!opened) reject(new TransportRefused("Couldn’t connect the call."))
          else if (!closing) closedListener()
        })
        // Ended while it opened: the socket goes, and the connect fails.
        const aborted = () => {
          closing = true
          socket.close()
          reject(new TransportRefused("Couldn’t connect the call."))
        }
        signal.addEventListener("abort", aborted, { once: true })
        socket.addEventListener("open", () => {
          signal.removeEventListener("abort", aborted)
          if (closing) return
          opened = true
          resolve({
            send: (event) => socket.send(JSON.stringify(event)),
            onEvent: (next) => (listener = next),
            onClosed: (next) => (closedListener = next),
            levels: () => ({ input: 0, output: 0 }),
            setMicrophone: () => {},
            setSpeaker: () => {},
            close: () => {
              closing = true
              socket.close()
            },
          })
        })
      })
    },
  }
}
