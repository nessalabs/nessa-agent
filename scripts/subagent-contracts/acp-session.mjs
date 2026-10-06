/** Bounded ACP framing and request supervision for the direct subagent probe. */
import { startProbeProcess, stopProbeProcess } from "./processes.mjs"

export const MAX_RECORD_BYTES = 8 * 1024 * 1024
const MAX_RECORDS = 10_000

/** Refuse a line before decoding/parsing, including a line without a newline. */
export function boundedFrames(onFrame, onFailure, { maxBytes = MAX_RECORD_BYTES } = {}) {
  let remaining = maxBytes
  let parts = []
  let failed = false
  const fail = (code) => {
    if (failed) return
    failed = true
    parts = []
    onFailure({ code })
  }
  const line = () => {
    const bytes = Buffer.concat(parts)
    parts = []
    if (!bytes.length || bytes.toString("utf8").trim() === "") return
    let frame
    try {
      frame = JSON.parse(bytes.toString("utf8"))
    } catch {
      return fail("invalid_frame")
    }
    if (
      frame === null ||
      typeof frame !== "object" ||
      Array.isArray(frame) ||
      frame.jsonrpc !== "2.0" ||
      (Object.hasOwn(frame, "method")
        ? typeof frame.method !== "string" ||
          (frame.params !== undefined &&
            (frame.params === null ||
              typeof frame.params !== "object" ||
              Array.isArray(frame.params)))
        : !Object.hasOwn(frame, "id") ||
          Object.hasOwn(frame, "result") === Object.hasOwn(frame, "error"))
    )
      return fail("invalid_frame")
    if (
      Object.hasOwn(frame, "id") &&
      !(typeof frame.id === "string" || Number.isSafeInteger(frame.id))
    )
      return fail("invalid_frame")
    try {
      onFrame(frame)
    } catch {
      fail("frame_handler_failed")
    }
  }
  return {
    take(chunk) {
      if (failed) return
      if (chunk.length > remaining) return fail("recording_limit")
      remaining -= chunk.length
      let start = 0
      for (let at = 0; at < chunk.length; at++) {
        if (chunk[at] !== 10) continue
        if (at > start) parts.push(Buffer.from(chunk.subarray(start, at)))
        line()
        if (failed) return
        start = at + 1
      }
      if (start < chunk.length) parts.push(Buffer.from(chunk.subarray(start)))
    },
    end() {
      if (!failed && parts.length) line()
    },
    stop() {
      failed = true
      parts = []
    },
  }
}

/** One cleanup owner survives callback failure, request timeout and leader exit. */
export function startProbeSession(
  command,
  args,
  { workspace, env, maxBytes = MAX_RECORD_BYTES, cleanupOptions } = {},
) {
  const owned = startProbeProcess(command, args, { cwd: workspace, env })
  const pending = new Map()
  const records = []
  let recordedBytes = 0
  let failure
  let next = 1
  let reader
  const fail = (error) => {
    failure ??= error
    for (const item of pending.values()) {
      clearTimeout(item.timer)
      item.reject(failure)
    }
    pending.clear()
    reader?.stop()
    owned.child.stdout.destroy()
    void stopProbeProcess(owned, cleanupOptions)
  }
  const retain = (direction, frame) => {
    const bytes = Buffer.byteLength(JSON.stringify(frame))
    if (recordedBytes + bytes > maxBytes || records.length >= MAX_RECORDS) {
      fail({ code: "recording_limit" })
      return false
    }
    recordedBytes += bytes
    records.push({ direction, frame })
    return true
  }
  const send = (frame) => {
    if (!retain("to-agent", frame)) return
    owned.child.stdin.write(`${JSON.stringify(frame)}\n`)
  }
  reader = boundedFrames(
    (frame) => {
      if (!retain("from-agent", frame)) return
      if (Object.hasOwn(frame, "id") && !Object.hasOwn(frame, "method")) {
        const item = pending.get(frame.id)
        if (item) {
          clearTimeout(item.timer)
          pending.delete(frame.id)
          item.resolve(frame)
        }
      } else if (frame.method === "session/request_permission") {
        send({
          jsonrpc: "2.0",
          id: frame.id,
          result: { outcome: { outcome: "cancelled" } },
        })
      }
    },
    fail,
    { maxBytes },
  )
  owned.child.stdout.on("data", (chunk) => reader.take(chunk))
  owned.child.stdout.on("end", () => reader.end())
  owned.child.stdout.on("error", () => fail({ code: "provider_read_failed" }))
  owned.child.stdin.on("error", () => fail({ code: "provider_write_failed" }))
  owned.child.on("error", () => fail({ code: "spawn_failed" }))
  owned.child.on("close", () => {
    if (pending.size) fail({ code: "provider_exit" })
  })
  return {
    records,
    child: owned.child,
    request(method, params, budget = 45_000) {
      if (failure) return Promise.reject(failure)
      const id = next++
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => fail({ code: "timeout", method }), budget)
        pending.set(id, { resolve, reject, timer })
        try {
          send({ jsonrpc: "2.0", id, method, params })
        } catch {
          fail({ code: "provider_write_failed" })
        }
      })
    },
    async close() {
      for (const item of pending.values()) {
        clearTimeout(item.timer)
        item.reject({ code: "probe_closing" })
      }
      pending.clear()
      return stopProbeProcess(owned, cleanupOptions)
    },
  }
}
