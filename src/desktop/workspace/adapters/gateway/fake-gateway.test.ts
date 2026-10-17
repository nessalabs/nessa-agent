import { expect, it } from "vitest"
import { permissionTargets, viewEntryBounds, type ConversationApi } from "@nessa/client"
import { deferred, fakeGateway, questionEntry, reviewEntry } from "./fake-gateway"

const conversationId = "00000000-0000-4000-8000-000000000001"
const target = permissionTargets({
  conversationId,
  permissions: [
    {
      review: "a".repeat(32),
      executionId: "turn",
      permissionId: "review",
      options: [{ id: "allow" }],
    },
  ],
})[0]!

it("AR5: answer callbacks retain their SDK arguments when later calls arrive", async () => {
  const gateway = fakeGateway()
  const other = permissionTargets({
    conversationId,
    permissions: [
      {
        review: "b".repeat(32),
        executionId: "other-turn",
        permissionId: "other-review",
        options: [{ id: "deny" }],
      },
    ],
  })[0]!
  const wait = deferred()
  let captured: Parameters<ConversationApi["answer"]> | undefined
  gateway.onceAnswer(async (normal, args) => {
    await wait.promise
    captured = args
    return normal()
  })
  const first = gateway.client.conversation.answer(target, "allow", {
    requestId: "first",
  })
  await gateway.client.conversation.answer(other, "deny", { requestId: "second" })
  await gateway.client.conversation.create({ conversationId })
  wait.resolve()
  expect(await first).toEqual({ requestId: "answer", applied: true })
  expect(captured).toEqual([target, "allow", { requestId: "first" }])
  expect(
    gateway.calls.filter((call) => call.method === "answer").map((call) => call.args),
  ).toEqual([
    [target, "allow", { requestId: "first" }],
    [other, "deny", { requestId: "second" }],
  ])
})

// Checked by the application's tsc invocation; never executed by the test runner.
function obsoleteContract() {
  const gateway = fakeGateway()
  // @ts-expect-error Four raw arguments are not the SDK permission answer API.
  void gateway.client.conversation.answer(conversationId, "turn", "review", "allow")
  // @ts-expect-error Answer callbacks must return the SDK mutation result.
  gateway.onceAnswer(async () => ({ executionId: "obsolete" }))
  for (const call of gateway.calls) {
    if (call.method !== "answer") continue
    // @ts-expect-error The discriminant preserves the SDK target in diagnostics.
    const oldRecordedConversation: string = call.args[0]
    void oldRecordedConversation
  }
  gateway.onceAnswer(async (normal, [answerTarget]) => {
    // @ts-expect-error The first argument is the complete target, not a string ID.
    const oldConversationId: string = answerTarget
    void oldConversationId
    return normal()
  })
}
void obsoleteContract

it("lists a long tool name or message clipped as the gateway clips it", () => {
  const review = reviewEntry({
    executionId: "execution",
    permissionId: "permission",
    toolId: "tool",
    title: "Run it",
    toolName: `${"x".repeat(viewEntryBounds.toolNameBytes - 1)}é`,
    argumentsJson: "{}",
    origin: { kind: "harness" },
    ask: "tool",
    options: [{ id: "allow", label: "Allow", effect: "allow" }],
  })
  // The two-byte character would pass the bound, so the entry stops before it.
  expect(review.toolName).toBe("x".repeat(viewEntryBounds.toolNameBytes - 1))
  const question = questionEntry({
    executionId: "execution",
    questionId: "question",
    message: `"${"y".repeat(viewEntryBounds.messageBytes)}`,
    questions: [],
  })
  // A quote takes two bytes encoded, as the gateway counts it.
  expect(question.message).toBe(`"${"y".repeat(viewEntryBounds.messageBytes - 2)}`)
})
