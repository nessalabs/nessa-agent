import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { test } from "node:test"
import { observeAccessRefusals } from "./stress-review-answers.mjs"

function fixture() {
  const page = new EventEmitter(),
    observer = observeAccessRefusals(page, "ws://localhost:1234")
  const socket = new EventEmitter()
  socket.url = () => "ws://localhost:1234/session"
  page.emit("websocket", socket)
  const send = (params = { conversationId: "c", mode: "full", requestId: "cmd" }) =>
    socket.emit("framesent", {
      payload: JSON.stringify({
        type: "req",
        id: "1",
        method: "conversation.setApprovalMode",
        params,
      }),
    })
  const respond = (frame) =>
    socket.emit("framereceived", {
      payload: JSON.stringify({ type: "res", id: "1", ...frame }),
    })
  return { observer, socket, send, respond }
}
const refusal = {
  ok: false,
  error: { code: "approval_mode_unavailable", message: "unavailable" },
}
test("exact window Full request retains its typed refusal, never success or disappearance", () => {
  const f = fixture(),
    scope = f.observer.begin({ conversationId: "c", mode: "full" })
  f.send()
  f.respond(refusal)
  assert.equal(scope.refused(), true)
  assert.equal(scope.acknowledged(), false)
  assert.equal(scope.finish().code, "approval_mode_unavailable")
  f.observer.dispose()
})
test("wrong target, response id, success and missing response cannot confirm refusal", () => {
  for (const kind of ["target", "mode", "id", "success", "missing", "duplicate"]) {
    const f = fixture(),
      scope = f.observer.begin({ conversationId: "c", mode: "full" })
    f.send({
      conversationId: kind === "target" ? "other" : "c",
      mode: kind === "mode" ? "auto" : "full",
      requestId: "cmd",
    })
    if (kind === "duplicate") f.send()
    if (kind === "success")
      f.respond({ ok: true, payload: { requestId: "cmd", mode: "full" } })
    else if (kind !== "missing")
      f.respond({ ...refusal, ...(kind === "id" ? { id: "2" } : {}) })
    assert.equal(scope.refused(), false, kind)
    scope.finish()
    f.observer.dispose()
  }
})
