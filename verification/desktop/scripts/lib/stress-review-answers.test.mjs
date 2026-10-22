import assert from "node:assert/strict"
import { EventEmitter } from "node:events"
import { test } from "node:test"
import {
  observeReviewAnswers,
  reviewAnswerAcknowledged,
} from "./stress-review-answers.mjs"

const target = {
  conversationId: "conversation",
  executionId: "execution",
  review: "review",
  permissionId: "permission",
  optionId: "allow",
}
function fixture() {
  const page = new EventEmitter()
  const observer = observeReviewAnswers(page, "ws://127.0.0.1:1234", () => 100)
  const socket = (url = "ws://127.0.0.1:1234/session") => {
    const socket = new EventEmitter()
    socket.url = () => url
    page.emit("websocket", socket)
    return socket
  }
  const send = (socket, params = target, requestId = "command") =>
    socket.emit("framesent", {
      payload: JSON.stringify({
        type: "req",
        id: "1",
        method: "conversation.answer",
        params: { ...params, requestId },
      }),
    })
  const ack = (socket, payload = { requestId: "command", applied: true }) =>
    socket.emit("framereceived", {
      payload: JSON.stringify({ type: "res", id: "1", ok: true, payload }),
    })
  return { page, observer, socket, send, ack }
}
test("exact applied ACK confirms only the control action", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  f.ack(socket)
  assert.equal(scope.acknowledged(), true)
  assert.deepEqual(scope.finish(), {
    ...target,
    requestId: "command",
    outcome: "acknowledged",
    clickAttemptAt: 100,
    sentAt: 100,
    respondedAt: 100,
  })
  f.observer.dispose()
})
test("expiry without an answer and lost ACK do not confirm disappearance", () => {
  const f = fixture(),
    socket = f.socket()
  let scope = f.observer.begin(target)
  assert.equal(scope.acknowledged(), false)
  assert.equal(scope.finish().outcome, "not sent")
  scope = f.observer.begin(target)
  f.send(socket)
  assert.equal(scope.acknowledged(), false)
  assert.equal(scope.finish().outcome, "unconfirmed")
  f.observer.dispose()
})
test("refusal after expiry stays refused after a late success frame", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  socket.emit("framereceived", {
    payload: JSON.stringify({
      type: "res",
      id: "1",
      ok: false,
      error: {
        code: "stale_permission",
        message: "review expired",
        details: { selectionState: "consumed" },
      },
    }),
  })
  f.ack(socket)
  assert.equal(scope.acknowledged(), false)
  assert.equal(scope.finish().code, "stale_permission")
  f.observer.dispose()
})
test("mismatched target fields cannot supply this answer", () => {
  for (const field of Object.keys(target)) {
    const f = fixture(),
      socket = f.socket(),
      scope = f.observer.begin(target)
    f.send(socket, { ...target, [field]: "other" })
    f.ack(socket)
    assert.equal(scope.finish().outcome, "not sent", field)
    f.observer.dispose()
  }
})
test("socket identity prevents response id reuse from confirming another socket", () => {
  const f = fixture(),
    socket = f.socket(),
    other = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  f.ack(other)
  assert.equal(scope.acknowledged(), false)
  f.ack(socket)
  assert.equal(scope.acknowledged(), true)
  scope.finish()
  f.observer.dispose()
})
test("command identity and applied flag must match", () => {
  for (const payload of [
    { requestId: "other", applied: true },
    { requestId: "command", applied: false },
    { applied: true },
  ]) {
    const f = fixture(),
      socket = f.socket(),
      scope = f.observer.begin(target)
    f.send(socket)
    f.ack(socket, payload)
    assert.equal(scope.finish().outcome, "invalid acknowledgement")
    f.observer.dispose()
  }
})
test("duplicate exact commands cannot be hidden by either ACK", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  f.send(socket)
  f.ack(socket)
  assert.equal(scope.finish().outcome, "multiple commands")
  f.observer.dispose()
})
test("malformed command identities cannot acknowledge", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket, target, null)
  f.ack(socket, { requestId: null, applied: true })
  assert.equal(scope.acknowledged(), false)
  scope.finish()
  f.observer.dispose()
})
test("other endpoints and malformed frames are ignored; disposal detaches listeners", () => {
  const f = fixture(),
    other = f.socket("ws://other/session"),
    socket = f.socket(),
    scope = f.observer.begin(target)
  socket.emit("framesent", { payload: "not JSON" })
  f.send(other)
  f.ack(other)
  assert.equal(scope.finish().outcome, "not sent")
  f.observer.dispose()
  assert.equal(f.page.listenerCount("websocket"), 0)
  assert.equal(socket.listenerCount("framesent"), 0)
  assert.equal(socket.listenerCount("framereceived"), 0)
})

test("a duplicate after ACK revokes confirmation before the review disappears", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  f.ack(socket)
  assert.equal(scope.acknowledged(), true)
  f.send(socket)
  assert.equal(scope.acknowledged(), false)
  assert.equal(scope.finish().outcome, "multiple commands")
  f.observer.dispose()
})

test("canonical wire parser rejects ambiguous and malformed ACKs", () => {
  for (const raw of [
    '{"type":"res","id":"1","ok":true,"payload":{"requestId":"wrong","requestId":"command","applied":true}}',
    '{"type":"res","id":"1","ok":true,"payload":{"requestId":"wrong","request\\u0049d":"command","applied":true}}',
    '{"type":"res","id":"1","ok":true,"payload":{"requestId":"command","applied":true},"error":{"code":"failure","message":"refused"}}',
    '{"type":"res","id":"1","ok":true,"payload":{"requestId":"command","applied":true},"unknown":true}',
  ]) {
    const f = fixture(),
      socket = f.socket(),
      scope = f.observer.begin(target)
    f.send(socket)
    socket.emit("framereceived", { payload: raw })
    assert.equal(scope.acknowledged(), false, raw)
    assert.equal(scope.finish().outcome, "unconfirmed")
    f.observer.dispose()
  }
})

test("the step success gate rejects disappearance with expiry, lost ACK or a late duplicate", () => {
  for (const outcome of ["not sent", "unconfirmed", "refused", "multiple commands"]) {
    assert.equal(reviewAnswerAcknowledged({ outcome }, true), false)
  }
  assert.equal(reviewAnswerAcknowledged({ outcome: "acknowledged" }, false), false)
  assert.equal(reviewAnswerAcknowledged({ outcome: "acknowledged" }, true), true)
})

test("binary ACKs and extra receipt fields cannot confirm the browser command", () => {
  for (const payload of [
    Buffer.from(
      JSON.stringify({
        type: "res",
        id: "1",
        ok: true,
        payload: { requestId: "command", applied: true },
      }),
    ),
    JSON.stringify({
      type: "res",
      id: "1",
      ok: true,
      payload: { requestId: "command", applied: true, extra: true },
    }),
  ]) {
    const f = fixture(),
      socket = f.socket(),
      scope = f.observer.begin(target)
    f.send(socket)
    socket.emit("framereceived", { payload })
    assert.equal(scope.acknowledged(), false)
    scope.finish()
    f.observer.dispose()
  }
})

test("response envelope identity must match even with the exact command receipt", () => {
  const f = fixture(),
    socket = f.socket(),
    scope = f.observer.begin(target)
  f.send(socket)
  socket.emit("framereceived", {
    payload: JSON.stringify({
      type: "res",
      id: "other-envelope",
      ok: true,
      payload: { requestId: "command", applied: true },
    }),
  })
  assert.equal(scope.acknowledged(), false)
  f.ack(socket)
  assert.equal(scope.acknowledged(), true)
  assert.equal(scope.finish().outcome, "acknowledged")
  f.observer.dispose()
})
