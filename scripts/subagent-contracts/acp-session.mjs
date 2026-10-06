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
  // EOF and explicit sealing share one rule: only a newline admits a frame.
  const complete = () => {
    if (!failed && parts.length && Buffer.concat(parts).toString("utf8").trim())
      fail("incomplete_frame_at_seal")
    parts = []
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
    end: complete,
    complete,
    stop() {
      failed = true
      parts = []
    },
  }
}

/** The probe owns one sequential RPC and its independently admitted session. */
export function startProbeSession(
  command,
  args,
  { workspace, env, maxBytes = MAX_RECORD_BYTES, cleanupOptions } = {},
) {
  const owned = startProbeProcess(command, args, { cwd: workspace, env })
  const records = []
  let recordedBytes = 0
  let failure
  let next = 1
  let active
  let phase = "starting"
  let reader
  let snapshot
  const admission = {}
  const fail = (error) => {
    if (phase === "sealed") return
    failure ??= error
    phase = "failed"
    if (active) {
      clearTimeout(active.timer)
      active.reject(failure)
      active = undefined
    }
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
    if (retain("to-agent", frame)) owned.child.stdin.write(`${JSON.stringify(frame)}\n`)
  }
  reader = boundedFrames(
    (frame) => {
      if (!retain("from-agent", frame)) return
      if (Object.hasOwn(frame, "id") && !Object.hasOwn(frame, "method")) {
        if (!active || active.id !== frame.id)
          return fail({ code: "unsolicited_response" })
        const request = active
        if (frame.error)
          return fail({
            code: {
              initialize: "initialize_rejected",
              "session/new": "session_rejected",
              "session/prompt": "prompt_rejected",
              "session/close": "close_rejected",
            }[request.method],
            rpcCode: frame.error.code,
          })
        if (request.method === "initialize") phase = "initialized"
        else if (request.method === "session/new") {
          if (typeof frame.result?.sessionId !== "string" || !frame.result.sessionId)
            return fail({ code: "session_invalid" })
          admission.opening = {
            requestId: request.id,
            responseId: frame.id,
            sessionId: frame.result.sessionId,
          }
          phase = "open"
        } else if (request.method === "session/prompt") {
          if (
            ![
              "end_turn",
              "cancelled",
              "max_tokens",
              "max_turn_requests",
              "refusal",
            ].includes(frame.result?.stopReason)
          )
            return fail({ code: "prompt_invalid" })
          admission.prompt = {
            requestId: request.id,
            responseId: frame.id,
            sessionId: request.sessionId,
            stopReason: frame.result.stopReason,
          }
          phase = "terminal"
        } else if (request.method === "session/close") {
          if (
            !frame.result ||
            typeof frame.result !== "object" ||
            Array.isArray(frame.result) ||
            Object.keys(frame.result).length
          )
            return fail({ code: "close_invalid" })
          admission.close = {
            requestId: request.id,
            responseId: frame.id,
            sessionId: request.sessionId,
            acknowledged: true,
          }
          phase = "close_answered"
        }
        clearTimeout(request.timer)
        active = undefined
        request.resolve(frame)
      } else if (
        frame.method === "session/update" &&
        frame.params?.update?._meta?.codex?.collaboration
      ) {
        if (phase !== "prompting")
          return fail({
            code: admission.prompt ? "activity_after_terminal" : "premature_activity",
          })
        const { sessionId, update } = frame.params
        if (
          sessionId !== admission.opening.sessionId ||
          update.rawInput?.senderThreadId !== sessionId ||
          update._meta.codex.collaboration.senderThreadId !== sessionId
        )
          fail({ code: "admitted_session_mismatch" })
      } else if (frame.method === "session/request_permission") {
        if (phase !== "prompting") return fail({ code: "premature_permission" })
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
    if (active) fail({ code: "provider_exit" })
  })
  return {
    request(method, params, budget = 45_000) {
      if (failure) return Promise.reject(failure)
      const allowed = {
        starting: "initialize",
        initialized: "session/new",
        open: "session/prompt",
        terminal: "session/close",
      }
      if (active || !Object.hasOwn(allowed, phase) || allowed[phase] !== method)
        return Promise.reject({ code: "request_out_of_order" })
      if (
        (method === "session/prompt" || method === "session/close") &&
        params.sessionId !== admission.opening.sessionId
      )
        return Promise.reject({ code: "admitted_session_mismatch" })
      const id = next++
      phase = {
        initialize: "starting",
        "session/new": "opening",
        "session/prompt": "prompting",
        "session/close": "closing",
      }[method]
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => fail({ code: "timeout", method }), budget)
        active = { id, method, sessionId: params.sessionId, resolve, reject, timer }
        try {
          send({ jsonrpc: "2.0", id, method, params })
        } catch {
          fail({ code: "provider_write_failed" })
        }
      })
    },
    seal() {
      if (snapshot) return snapshot
      reader.complete()
      if (failure) throw failure
      if (phase !== "close_answered" || active) throw { code: "recording_not_complete" }
      snapshot = Object.freeze({
        kind: "sealed-acp-probe",
        admissionJson: JSON.stringify(admission),
        recordingJson: JSON.stringify(records),
      })
      phase = "sealed"
      reader.stop()
      records.length = 0
      return snapshot
    },
    async close() {
      if (active) fail({ code: "probe_closing" })
      return stopProbeProcess(owned, cleanupOptions)
    },
  }
}
