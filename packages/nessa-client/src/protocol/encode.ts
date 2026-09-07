import type { ReqFrame } from "./types.js"

/** Build a client request frame (`type: "req"`). */
export function buildRequestFrame(id: string, method: string, params: unknown): ReqFrame {
  if (typeof params !== "object" || params === null || Array.isArray(params))
    throw new Error("RPC params must be an object")
  return { type: "req", id, method, params: params as Record<string, unknown> }
}

/** Serialize a frame to the JSON text sent on the WebSocket. */
export function encodeWireMessage(frame: ReqFrame): string {
  return JSON.stringify(frame)
}
