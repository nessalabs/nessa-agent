/**
 * The `postMessage` transport between the host page and one app's sandbox
 * proxy frame (MCP Apps, *Transport Layer*, *Sandbox proxy*).
 *
 * A message is read only when it comes from **this** frame's window and
 * carries the sandbox's origin: every other frame on the page — another
 * app's, a stale one — and every other origin is dropped before its data is
 * touched. What passes is read once, into a copy (`model/json-rpc.ts`), and
 * typed (`model/messages.ts`); the bridge never sees the event.
 *
 * Messages go to the frame's window addressed to the sandbox's origin, so a
 * frame that has been navigated anywhere else receives nothing.
 */
import { readEnvelope, type Outgoing } from "../../model/json-rpc"
import { readFromFrame, type FromFrame } from "../../model/messages"

export interface FrameTransport {
  post(message: Outgoing): void
  /** Stops listening; nothing is delivered or posted after. */
  close(): void
}

export function frameTransport(
  frame: HTMLIFrameElement,
  sandboxOrigin: string,
  deliver: (message: FromFrame) => void,
): FrameTransport {
  let open = true
  const view = frame.ownerDocument.defaultView
  const listener = (event: MessageEvent) => {
    // Identity first, then origin; the data only after both.
    if (!open || event.source === null || event.source !== frame.contentWindow) return
    if (event.origin !== sandboxOrigin) return
    deliver(readFromFrame(readEnvelope(event.data)))
  }
  view?.addEventListener("message", listener)
  return {
    post(message) {
      if (open) frame.contentWindow?.postMessage(message, sandboxOrigin)
    },
    close() {
      open = false
      view?.removeEventListener("message", listener)
    },
  }
}
