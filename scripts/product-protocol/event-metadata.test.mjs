import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import Ajv2020 from "ajv/dist/2020.js"
import { eventMetadataRules } from "./event-metadata.mjs"

const schema = JSON.parse(
  readFileSync(new URL("../../protocol/product/v1.json", import.meta.url)),
)
const ajv = new Ajv2020({ strict: false })
ajv.addSchema(schema)
const validate = ajv.getSchema(`${schema.$id}#/$defs/ConversationEvent`)
const rules = eventMetadataRules(schema)
const base = { id: "one", cursor: "one" }

test("event filter schema distinguishes omitted options from malformed supplied values", () => {
  const validateOptions = ajv.getSchema(
    `${schema.$id}#/$defs/ConversationSubscribeEventsParams`,
  )
  const conversationId = "00000000-0000-4000-8000-000000000001"
  for (const options of [
    {},
    { kinds: ["turn.ended"] },
    { kinds: schema.$defs.ConversationEventKind.enum },
    { replay: false },
  ])
    assert.equal(validateOptions({ conversationId, ...options }), true)
  for (const options of [
    { kinds: [] },
    { kinds: ["turn.ended", "turn.ended"] },
    { kinds: [...schema.$defs.ConversationEventKind.enum, "turn.ended"] },
    { kinds: null },
    { kinds: "turn.ended" },
    { kinds: ["future.unknown"] },
    { after: null },
    { replay: null },
    { after: "cursor", replay: false },
  ])
    assert.equal(
      validateOptions({ conversationId, ...options }),
      false,
      JSON.stringify(options),
    )
})
const valid = [
  { kind: "turn.accepted", executionId: "turn", status: "accepted" },
  { kind: "turn.started", executionId: "turn", status: "running" },
  { kind: "turn.ended", executionId: "turn", status: "settled", outcome: "completed" },
  { kind: "tool.updated", executionId: "turn", toolId: "tool" },
  {
    kind: "tool.updated",
    executionId: "turn",
    toolId: "tool",
    name: "shell",
    status: "running",
  },
  { kind: "state.changed", status: "opened" },
  {
    kind: "state.changed",
    executionId: "turn",
    status: "settlement_updated",
    outcome: "failed",
  },
  ...[
    "queued",
    "injected",
    "settled",
    "cancelled",
    "waiting_permission",
    "permission_cancelled",
    "permission_declined",
    "permission_deadline_answered",
    "waiting_question",
    "question_closed",
    "stop_requested",
  ].map((status) => ({ kind: "state.changed", executionId: "turn", status })),
]
const invalid = [
  { kind: "turn.ended" },
  { kind: "turn.ended", executionId: "turn", status: "settled" },
  { kind: "turn.started", executionId: "turn", status: "settled" },
  { kind: "turn.accepted", executionId: "turn", status: "accepted", toolId: "tool" },
  { kind: "tool.updated", executionId: "turn" },
  { kind: "tool.updated", toolId: "tool" },
  { kind: "tool.updated", executionId: "turn", toolId: "tool", outcome: "completed" },
  { kind: "state.changed", status: "opened", executionId: "turn" },
  { kind: "state.changed", status: "settlement_updated", executionId: "turn" },
  { kind: "state.changed", status: "waiting_question" },
  { kind: "state.changed", status: "queued", executionId: "turn", outcome: "failed" },
]
function publishedAccepts(event) {
  return rules.some(
    (rule) =>
      rule.kind === event.kind &&
      rule.required.every((field) => Object.hasOwn(event, field)) &&
      rule.forbidden.every((field) => !Object.hasOwn(event, field)) &&
      Object.entries(rule.values).every(
        ([field, values]) =>
          !Object.hasOwn(event, field) || values.includes(event[field]),
      ),
  )
}
test("canonical semantic shapes and published declarations agree on sparse and contradictory metadata", () => {
  for (const [cases, expected] of [
    [valid, true],
    [invalid, false],
  ])
    for (const fields of cases) {
      const event = { ...base, ...fields }
      assert.equal(validate(event), expected, JSON.stringify(event))
      assert.equal(publishedAccepts(event), expected, JSON.stringify(event))
    }
})
test("changing the canonical required field changes published validation data", () => {
  const changed = structuredClone(schema)
  changed.$defs.ConversationEvent.allOf[0].then.required.push("name")
  assert.ok(eventMetadataRules(changed)[0].required.includes("name"))
  const incomplete = structuredClone(schema)
  incomplete.$defs.ConversationEvent.allOf.pop()
  assert.throws(() => eventMetadataRules(incomplete), /Every semantic event kind/)
})

test("unsupported conditional constraints fail publication rather than disappearing", () => {
  for (const mutate of [
    (branch) => {
      branch.if.required = ["status"]
    },
    (branch) => {
      branch.if.properties.status = { const: "accepted" }
    },
    (branch) => {
      branch.then.not = { required: ["toolId"] }
    },
    (branch) => {
      branch.then.oneOf = [structuredClone(branch.then)]
    },
    (branch) => {
      branch.then.properties.status.pattern = "^accepted$"
    },
    (branch) => {
      branch.then.not.anyOf[0].properties = { toolId: { const: "tool" } }
    },
  ]) {
    const changed = structuredClone(schema)
    mutate(changed.$defs.ConversationEvent.allOf[0])
    assert.throws(() => eventMetadataRules(changed), /Unsupported|Invalid forbidden/)
  }
})
