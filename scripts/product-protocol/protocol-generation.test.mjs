/** Generation refuses a late unsupported input before publishing any artifact. */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import {
  cpSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { coreWireContract } from "./core-contract.mjs"

const root = fileURLToPath(new URL("../../", import.meta.url))
const contract = coreWireContract(root)
const genericOutputs = [
  "packages/nessa-client/src/generated/protocol.ts",
  "packages/nessa-client/src/generated/catalog.ts",
  "crates/nessa-server/src/protocol/generated_catalog.rs",
  "crates/nessa-server/src/protocol/generated_types.rs",
]
const productOutputs = [
  "crates/nessa-auth/src/domain/pairing/value_objects/wire_values.rs",
  "protocol/product/pairing-values.generated.json",
  "packages/nessa-client/src/generated/product.ts",
  "crates/nessa-server/src/product/generated.rs",
  "crates/nessa-server/src/product_contract/generated.rs",
]
function fixture(run) {
  const path = mkdtempSync(join(tmpdir(), "nessa-generation-"))
  try {
    for (const name of [
      "protocol",
      "scripts/product-protocol",
      "scripts/generate-protocol-types.mjs",
      "scripts/generate-product-protocol.mjs",
      "prettier.config.js",
      "package.json",
      "crates/nessa-sdk/src/infrastructure/session_storage/stream_fact.rs",
      "crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs",
      "crates/nessa-server/src/protocol/encode.rs",
      "crates/nessa-auth/src/domain/pairing/value_objects/wire-values.json",
    ]) {
      mkdirSync(dirname(join(path, name)), { recursive: true })
      cpSync(join(root, name), join(path, name), { recursive: true })
    }
    symlinkSync(join(root, "node_modules"), join(path, "node_modules"))
    // This fixture substitutes only process acquisition; values come from the resolved owner.
    writeFileSync(
      join(path, "scripts/product-protocol/core-contract.mjs"),
      `export {applyCoreWireBounds} from ${JSON.stringify(join(root, "scripts/product-protocol/core-contract.mjs"))}; export function coreWireContract(){return ${JSON.stringify(contract)}}`,
    )
    for (const name of [...genericOutputs, ...productOutputs]) {
      mkdirSync(dirname(join(path, name)), { recursive: true })
      writeFileSync(join(path, name), "unpublished sentinel\n")
    }
    run(path)
  } finally {
    rmSync(path, { recursive: true, force: true })
  }
}
function edit(path, name, change) {
  const target = join(path, name)
  const value = JSON.parse(readFileSync(target, "utf8"))
  change(value)
  writeFileSync(target, JSON.stringify(value))
}
function generate(path, script, args = []) {
  return spawnSync(process.execPath, [`scripts/${script}.mjs`, ...args], {
    cwd: path,
    encoding: "utf8",
    timeout: 120_000,
    env: Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith("NESSA_PROTOCOL_")),
    ),
  })
}
function unchanged(path, names, run) {
  const before = names.map((name) => readFileSync(join(path, name)))
  const result = run()
  assert.notEqual(result.status, 0, result.stdout)
  assert.equal(result.error, undefined)
  names.forEach((name, index) =>
    assert.deepEqual(readFileSync(join(path, name)), before[index], name),
  )
  return result
}

test("late generic Rust refusal preserves every artifact", () =>
  fixture((path) => {
    edit(path, "protocol/schemas/v1/common.json", (schema) => {
      schema.$defs.LateUnsupported = { type: "boolean" }
    })
    const result = unchanged(path, genericOutputs, () =>
      generate(path, "generate-protocol-types"),
    )
    assert.match(result.stderr, /Unsupported|unsupported/)
  }))
test("duplicate frame/common identity refuses before writes", () =>
  fixture((path) => {
    edit(path, "protocol/schemas/v1/frames.json", (schema) => {
      schema.$defs.GatewayError = { type: "boolean" }
    })
    const result = unchanged(path, genericOutputs, () =>
      generate(path, "generate-protocol-types"),
    )
    assert.match(result.stderr, /Ambiguous common\/frame schema definition/)
  }))
test("late product grammar refusal preserves schema and artifacts", () =>
  fixture((path) => {
    edit(path, "protocol/product/v1.json", (schema) => {
      schema.$defs.ProductSessionReady.properties.methods.maxItems = 0
      schema.$defs.ProductSessionReady.properties.protocol = {
        type: "string",
        const: "only",
      }
    })
    const result = unchanged(path, ["protocol/product/v1.json", ...productOutputs], () =>
      generate(path, "generate-product-protocol"),
    )
    assert.match(result.stderr, /Unsupported wire schema/)
  }))
test("current supported generators publish complete outputs", () =>
  fixture((path) => {
    for (const script of ["generate-protocol-types", "generate-product-protocol"]) {
      const result = generate(path, script)
      assert.equal(result.status, 0, result.stderr)
    }
    for (const name of [...genericOutputs, ...productOutputs])
      assert.notEqual(readFileSync(join(path, name), "utf8"), "unpublished sentinel\n")
  }))

test("passive timing publishes changed phase values and their derived client floor", () =>
  fixture((path) => {
    edit(path, "protocol/product/v1.json", (schema) => {
      schema["x-passiveReadTiming"] = {
        readTimeoutMs: 61,
        deliveryTimeoutMs: 73,
        clientAllowanceMs: 89,
      }
    })
    const result = generate(path, "generate-product-protocol")
    assert.equal(result.status, 0, result.stderr)
    const ts = readFileSync(
      join(path, "packages/nessa-client/src/generated/product.ts"),
      "utf8",
    )
    const rust = readFileSync(
      join(path, "crates/nessa-server/src/product/generated.rs"),
      "utf8",
    )
    assert.match(ts, /readTimeoutMs: 61/)
    assert.match(ts, /deliveryTimeoutMs: 73/)
    assert.match(ts, /clientAllowanceMs: 89/)
    assert.match(ts, /minRequestTimeoutMs: 223/)
    for (const [name, value] of [
      ["READ_TIMEOUT_MS", 61],
      ["DELIVERY_TIMEOUT_MS", 73],
      ["CLIENT_ALLOWANCE_MS", 89],
      ["MIN_REQUEST_TIMEOUT_MS", 223],
    ])
      assert.match(rust, new RegExp(`PASSIVE_${name}: u64 = ${value};`))
  }))

for (const [name, value] of [
  ["readTimeoutMs", undefined],
  ["readTimeoutMs", 0],
  ["readTimeoutMs", "10000"],
  ["deliveryTimeoutMs", -1],
  ["deliveryTimeoutMs", 1.5],
  ["clientAllowanceMs", null],
  ["clientAllowanceMs", 2_147_483_647],
])
  test(`invalid passive ${name} ${value} preserves unpublished artifacts`, () =>
    fixture((path) => {
      edit(path, "protocol/product/v1.json", (schema) => {
        schema["x-passiveReadTiming"][name] = value
      })
      const result = unchanged(
        path,
        ["protocol/product/v1.json", ...productOutputs],
        () => generate(path, "generate-product-protocol"),
      )
      assert.match(result.stderr, /Invalid passive read timing|deadline exceeds/)
    }))

test("pairing owner changes derive auth constants and published shape before drift check", () =>
  fixture((path) => {
    edit(
      path,
      "crates/nessa-auth/src/domain/pairing/value_objects/wire-values.json",
      (values) => {
        values.identityBytes += 1
        values.deviceKeyBytes += 2
        values.manualCodeBytes += 3
      },
    )
    const values = JSON.parse(
      readFileSync(
        join(path, "crates/nessa-auth/src/domain/pairing/value_objects/wire-values.json"),
        "utf8",
      ),
    )
    const result = generate(path, "generate-product-protocol")
    assert.equal(result.status, 0, result.stderr)
    const rust = readFileSync(
      join(path, "crates/nessa-auth/src/domain/pairing/value_objects/wire_values.rs"),
      "utf8",
    )
    assert.match(rust, new RegExp(`IDENTITY_BYTES: usize = ${values.identityBytes};`))
    assert.match(rust, new RegExp(`DEVICE_KEY_BYTES: usize = ${values.deviceKeyBytes};`))
    assert.match(
      rust,
      new RegExp(`MANUAL_CODE_BYTES: usize = ${values.manualCodeBytes};`),
    )
    const publication = JSON.parse(
      readFileSync(join(path, "protocol/product/pairing-values.generated.json"), "utf8"),
    )
    assert.equal(publication.$defs.InvitationId.maxItems, values.identityBytes)
    assert.equal(publication.$defs.DeviceKey.maxItems, values.deviceKeyBytes)
    assert.equal(
      publication.$defs.ManualCodeDisplay.maxLength,
      values.manualCodeBytes + 1,
    )
    assert.equal(generate(path, "generate-product-protocol", ["--check"]).status, 0)
    writeFileSync(
      join(path, "crates/nessa-auth/src/domain/pairing/value_objects/wire_values.rs"),
      "stale publication\n",
    )
    const refused = unchanged(path, productOutputs, () =>
      generate(path, "generate-product-protocol", ["--check"]),
    )
    assert.match(refused.stderr, /Generated product protocol is stale/)
  }))

test("invalid pairing publication refuses every output before writes", () =>
  fixture((path) => {
    edit(
      path,
      "crates/nessa-auth/src/domain/pairing/value_objects/wire-values.json",
      (values) => {
        values.deviceKeyBytes = 0
      },
    )
    const refused = unchanged(path, ["protocol/product/v1.json", ...productOutputs], () =>
      generate(path, "generate-product-protocol"),
    )
    assert.match(refused.stderr, /Invalid pairing owner publication/)
  }))
