import assert from "node:assert/strict"
import test from "node:test"
import { applyCoreWireBounds } from "./core-contract.mjs"

test("annotated wire bounds consume published owner values through nested schemas", () => {
  const schema = {
    fields: [
      { type: "string", "x-core-utf8-bound": "id_max_utf8_bytes", "x-utf8MaxBytes": 128 },
    ],
  }
  applyCoreWireBounds(schema, { id_max_utf8_bytes: 17 })
  assert.equal(schema.fields[0]["x-utf8MaxBytes"], 17)
  applyCoreWireBounds(schema, { id_max_utf8_bytes: 23 })
  assert.equal(schema.fields[0]["x-utf8MaxBytes"], 23)
})

test("unknown or inherited owner keys do not supply wire bounds", () => {
  for (const contract of [
    {},
    Object.create({ id_max_utf8_bytes: 128 }),
    { id_max_utf8_bytes: 0 },
    { id_max_utf8_bytes: "128" },
  ]) {
    assert.throws(
      () => applyCoreWireBounds({ "x-core-utf8-bound": "id_max_utf8_bytes" }, contract),
      /Unknown core wire bound/,
    )
  }
})

test("catalogue transport ceilings derive from the published owner and base64 representation", () => {
  const schema = {
    count: { "x-core-maximum-bound": "catalogue_max_entries" },
    entries: { "x-core-max-items-bound": "catalogue_max_entries" },
    payload: { "x-core-base64-bound": "catalogue_max_payload_bytes" },
  }
  applyCoreWireBounds(schema, {
    catalogue_max_entries: 3,
    catalogue_max_payload_bytes: 5,
  })
  assert.equal(schema.count.maximum, 3)
  assert.equal(schema.entries.maxItems, 3)
  assert.equal(schema.payload.maxLength, 8)
  applyCoreWireBounds(schema, {
    catalogue_max_entries: 7,
    catalogue_max_payload_bytes: 7,
  })
  assert.equal(schema.count.maximum, 7)
  assert.equal(schema.entries.maxItems, 7)
  assert.equal(schema.payload.maxLength, 12)
})
