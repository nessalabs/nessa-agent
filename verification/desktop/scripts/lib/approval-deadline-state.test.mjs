import assert from "node:assert/strict"
import { test } from "node:test"
import { tsImport } from "tsx/esm/api"

const { armed, conversation, reason, shown, waitMs } = await tsImport(
  "../../fixtures/approval-deadline/state.ts",
  import.meta.url,
)
const { conversationView } = await tsImport(
  "../../../../packages/nessa-client/src/protocol/conversation-validate.ts",
  import.meta.url,
)

test("the actual deadline fixture publishes its armed wait and typed expired notice", () => {
  const gatewayNow = 60_000
  const review = armed(gatewayNow + waitMs)
  const pending = shown(1, review)
  assert.equal(waitMs, 10_000)
  assert.equal(pending.messages[0].status, "running")
  assert.equal(pending.messages[0].parts.length, 0)
  assert.deepEqual(pending.permissions[0].deadline, {
    waitMs,
    dueAtMs: gatewayNow + waitMs,
    effect: "deny",
    reason,
    stopsTurn: false,
  })

  const expired = shown(2, undefined)
  assert.equal(expired.messages[0].status, "completed")
  assert.equal(expired.permissions.length, 0)
  assert.deepEqual(expired.messages[0].parts, [
    {
      offset: 0,
      kind: "local_notice",
      text: `Denied automatically: ${reason}`,
      toolId: "",
      noticeId: review.permissionId,
      noticeKind: "deadline",
    },
  ])
})

test("the current client refuses the original fixture's untyped local notice", () => {
  const expired = structuredClone(shown(2, undefined))
  delete expired.messages[0].parts[0].noticeKind
  assert.throws(
    () => conversationView(expired, conversation),
    /Conversation local notice has no source/,
  )
})
