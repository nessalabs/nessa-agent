import assert from "node:assert/strict"
import test from "node:test"
import Ajv2020 from "ajv/dist/2020.js"
import { rustWireShapes } from "./rust-wire-shapes.mjs"
import { compareGeneratedRust } from "./wire-shape-runtime.mjs"

const closed = (properties, required = Object.keys(properties)) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
})
const array = (items) => ({ type: "array", items, maxItems: 3 })
const compile = (schema) => rustWireShapes({ Input: schema }, ["Input"])
const reviewerCases = [
  {
    type: "object",
    properties: { owned: { type: "string" } },
    additionalProperties: { type: "string" },
  },
  { type: ["string", "null"], const: "only" },
  { minimum: 3 },
  { type: "object", properties: { blocked: false }, required: ["blocked"] },
  { type: "array", minItems: 1, items: false },
  { enum: [""] },
]

test("six reviewer counterexamples refuse at root, nested object/array and reference", () => {
  for (const schema of reviewerCases) {
    for (const wrapped of [schema, closed({ child: schema }), array(schema)])
      assert.throws(() => compile(wrapped), /Unsupported/)
    assert.throws(
      () => rustWireShapes({ Input: { $ref: "#/$defs/Bad" }, Bad: schema }, ["Input"]),
      /Unsupported/,
    )
  }
})

test("grammar refuses malformed nodes, values and incompatible keywords", () => {
  for (const schema of [
    closed({ blocked: false }),
    { ...closed({}), additionalProperties: { type: "string" } },
    array(false),
    null,
    true,
    false,
    [],
    "string",
    {},
    { description: "only" },
    { type: "string", minimum: 0 },
    { type: "string", const: "only" },
    { type: "string", enum: [""] },
    { type: "string", minLength: -1 },
    { type: "string", maxLength: 1.5 },
    { type: "string", maxLength: Infinity },
    { type: "string", minLength: 2, maxLength: 1 },
    { type: "boolean", const: true },
    { type: "integer" },
    { type: "integer", minimum: -1 },
    { type: "integer", minimum: 0, maximum: Number.MAX_SAFE_INTEGER + 1 },
    { type: "integer", const: 1, minimum: 0 },
    { type: "integer", const: false },
    { type: "integer", const: -1 },
    { type: "null" },
    { type: ["null", "integer"], minimum: 0 },
    { type: ["integer", "null"], minimum: 0, const: 1 },
    { type: "array", maxItems: 3 },
    { type: "array", items: { type: "string" } },
    { ...array({ type: "string" }), minItems: 4 },
    { ...closed({ field: { type: "string" } }), additionalProperties: true },
    { ...closed({ field: { type: "string" } }), required: ["missing"] },
    { ...closed({ field: { type: "string" } }), required: ["field", "field"] },
    { ...closed({}), required: "field" },
    { ...closed({}), properties: [] },
    { type: "string", description: 7 },
    { type: "string", tsType: "unknown" },
    { tsType: "other" },
    { $ref: "#/$defs/Text", maxLength: 1 },
    { $ref: false },
    { $ref: "foreign.json#/$defs/Text" },
    { const: "\ud800" },
    closed({ ["\ud800"]: { type: "string" } }),
  ]) {
    assert.throws(
      () => rustWireShapes({ Input: schema, Text: { type: "string" } }, ["Input"]),
      /Unsupported/,
    )
  }
})

test("references and required names consume only owned declarations", () => {
  const inherited = Object.create({ Text: { type: "string" } })
  inherited.Input = { $ref: "#/$defs/Text" }
  assert.throws(() => rustWireShapes(inherited, ["Input"]), /Unknown owned/)
  assert.throws(() => rustWireShapes({}, ["constructor"]), /Unknown owned/)
  assert.throws(
    () => rustWireShapes({ Input: { $ref: "#/$defs/Input" } }, ["Input"]),
    /cyclic/,
  )
  assert.throws(
    () =>
      rustWireShapes(
        { Input: { $ref: "common.json#/$defs/Text" }, Text: { type: "string" } },
        ["Input"],
        Object.create({ "common.json#/$defs/Text": "Text" }),
      ),
    /foreign/,
  )
  assert.throws(() => compile(closed({}, ["constructor"])), /required property/)
  assert.doesNotThrow(() => compile(closed({ constructor: { type: "string" } })))
  assert.throws(
    () =>
      rustWireShapes(
        { Input: { $ref: "#/$defs/BadName" }, BadName: { $ref: "#/$defs/BadName" } },
        ["Input"],
      ),
    /cyclic/,
  )
  assert.throws(
    () => rustWireShapes({ "bad-name": { type: "string" } }, ["bad-name"]),
    /identifier/,
  )
})

test("invalid definition and registry input refuse before traversal", () => {
  for (const input of [null, [], true]) {
    assert.throws(() => rustWireShapes(input, ["Input"]), /Unsupported/)
    assert.throws(
      () => rustWireShapes({ Input: { type: "string" } }, ["Input"], input),
      /Unsupported/,
    )
  }
  assert.throws(
    () => rustWireShapes({ Input: { type: "string" } }, "Input"),
    /Unsupported/,
  )
})

test("late reachable admission refusal produces no output", () => {
  let output = "unchanged"
  assert.throws(() => {
    output = rustWireShapes(
      { Good: { type: "string" }, Bad: closed({ child: { minimum: 3 } }) },
      ["Good", "Bad"],
    )
  }, /Unsupported/)
  assert.equal(output, "unchanged")
})

test("admitted grammar compiles and executes like Ajv on supported values", () => {
  const weird = 'quote"\\\u0001\n😀'
  const samples = [
    [{ tsType: "unknown" }, [null, false, 1, "", { nested: [] }]],
    [{ const: weird }, [weird, "other", null]],
    [{ type: "string", minLength: 1, maxLength: 2 }, ["a", "😀", "😀a", "", "abc", null]],
    [{ type: "string" }, ["", "x", 1, null]],
    [{ type: "boolean" }, [true, false, 0, null]],
    [{ type: "integer", minimum: 0, maximum: 3 }, [0, 3, 4, -1, 1.5, null]],
    [{ type: "integer", minimum: 0 }, [0, 9007199254740991, -1, false]],
    [{ type: "integer", const: 1 }, [1, 0, null]],
    [{ type: ["integer", "null"], minimum: 0, maximum: 3 }, [null, 0, 3, 4, "", -1]],
    [array({ type: "string" }), [[], ["a"], [1], ["a", "b", "c", "d"], null]],
    [
      closed({ [weird]: { type: "string" } }),
      [{ [weird]: "value" }, {}, { [weird]: 1 }, { [weird]: "value", extra: 1 }],
    ],
    [closed({}), [{}, { extra: 1 }, null]],
    [
      closed({ owned: { type: "string" }, optional: { type: "boolean" } }, ["owned"]),
      [{ owned: "" }, { owned: "x", optional: true }, { owned: "x", optional: 1 }, {}],
    ],
  ]
  const definitions = {},
    expected = [],
    invocations = []
  const ajv = new Ajv2020({ strict: false })
  for (const [index, [schema, inputs]] of samples.entries()) {
    const name = `Case${index}`
    definitions[name] = schema
    const validate = ajv.compile(schema)
    for (const input of inputs) {
      expected.push(Boolean(validate(input)))
      invocations.push({ name, input })
    }
  }
  // Actual admitted reference and nullable shapes must execute as well as compile.
  definitions.Referenced = { $ref: "#/$defs/Case2" }
  for (const input of ["a", "", null]) {
    expected.push(Boolean(ajv.compile(definitions.Case2)(input)))
    invocations.push({ name: "Referenced", input })
  }
  assert.deepEqual(
    compareGeneratedRust(
      rustWireShapes(definitions, Object.keys(definitions)),
      invocations,
    ),
    expected,
  )
})
