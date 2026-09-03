import type { ReqFrame } from "./types.js"

/** Build a client request frame (`type: "req"`). */
export function buildRequestFrame(id: string, method: string, params: unknown): ReqFrame {
  return { type: "req", id, method, params }
}

/** Serialize a frame to the JSON text sent on the WebSocket. */
export function encodeWireMessage(frame: ReqFrame): string {
  return JSON.stringify(frame)
}
