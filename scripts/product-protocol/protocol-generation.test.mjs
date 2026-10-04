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
  "crates/nessa-protocol/src/protocol/generated_catalog.rs",
  "crates/nessa-protocol/src/protocol/generated_types.rs",
]
const productOutputs = [
  "crates/nessa-auth/src/domain/pairing/value_objects/wire_values.rs",
  "protocol/product/pairing-values.generated.json",
  "packages/nessa-client/src/generated/product.ts",
  "crates/nessa-protocol/src/product/generated.rs",
  "crates/nessa-protocol/src/product_contract/generated.rs",
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
      "crates/nessa-protocol/src/protocol/encode.rs",
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
      join(path, "crates/nessa-protocol/src/product/generated.rs"),
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

test("MCP App call timing publishes its parts, the client's deadline, and the gateway's values", () =>
  fixture((path) => {
    edit(path, "protocol/product/v1.json", (schema) => {
      schema["x-mcpAppCallTiming"] = {
        reviewDeadlineMs: 307,
        callTimeoutMs: 61,
        readTimeoutMs: 13,
        clientAllowanceMs: 11,
      }
    })
    const result = generate(path, "generate-product-protocol")
    assert.equal(result.status, 0, result.stderr)
    const ts = readFileSync(
      join(path, "packages/nessa-client/src/generated/product.ts"),
      "utf8",
    )
    const contract = readFileSync(
      join(path, "crates/nessa-protocol/src/product_contract/generated.rs"),
      "utf8",
    )
    const published = ts.slice(ts.indexOf("export const mcpAppCallTiming"))
    assert.match(published, /reviewDeadlineMs: 307/)
    assert.match(published, /callTimeoutMs: 61/)
    assert.match(published, /readTimeoutMs: 13/)
    assert.match(published, /clientAllowanceMs: 11/)
    assert.match(published, /callDeadlineMs: 379/)
    assert.match(contract, /MCP_APP_REVIEW_DEADLINE_MS: u64 = 307;/)
    assert.match(contract, /MCP_APP_CALL_TIMEOUT_MS: u64 = 61;/)
    assert.match(contract, /MCP_APP_READ_TIMEOUT_MS: u64 = 13;/)
    assert.match(contract, /MCP_RESOURCE_TICKET_MS: u64 = \d+;/)
  }))

for (const [name, value] of [
  ["reviewDeadlineMs", undefined],
  ["reviewDeadlineMs", 0],
  ["callTimeoutMs", "60000"],
  ["callTimeoutMs", 1.5],
  ["readTimeoutMs", null],
  ["readTimeoutMs", 0],
  ["clientAllowanceMs", -1],
  ["clientAllowanceMs", 2_147_483_647],
  ["unknownMs", 1],
  ["readTimeoutMs", 60_001],
])
  test(`invalid MCP App call timing ${name} ${value} preserves unpublished artifacts`, () =>
    fixture((path) => {
      edit(path, "protocol/product/v1.json", (schema) => {
        schema["x-mcpAppCallTiming"][name] = value
      })
      const result = unchanged(
        path,
        ["protocol/product/v1.json", ...productOutputs],
        () => generate(path, "generate-product-protocol"),
      )
      assert.match(
        result.stderr,
        /Invalid MCP App call timing|unknown fields|deadline exceeds|outlasts a call/,
      )
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
    // The owner routes' schema takes the same widths, and their Rust fields
    // name the owner's constant rather than a number.
    const product = JSON.parse(
      readFileSync(join(path, "protocol/product/v1.json"), "utf8"),
    ).$defs
    assert.equal(
      product.PairingApproveParams.properties.invitationId.maxItems,
      values.identityBytes,
    )
    assert.equal(
      product.PairingApproveParams.properties.deviceKey.minItems,
      values.deviceKeyBytes,
    )
    assert.equal(
      product.PairingCreateResult.properties.code.maxLength,
      values.manualCodeBytes + 1,
    )
    const routes = readFileSync(
      join(path, "crates/nessa-protocol/src/product/generated.rs"),
      "utf8",
    )
    assert.match(routes, /pub device_key: \[u8; DeviceKey::LENGTH\],/)
    assert.match(routes, /pub invitation_id: \[u8; InvitationId::LENGTH\],/)
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

test("watch policy and identity publish the same schema owner to both languages", () =>
  fixture((path) => {
    const pattern = "^[a-z]+-[1-9][0-9]*$"
    edit(path, "protocol/product/v1.json", (schema) => {
      schema["x-changeWatchLimits"] = {
        globalOwners: 17,
        principalOwners: 5,
        recordTargets: 3,
        catalogueTargets: 2,
      }
      schema.$defs.ChangeWatchId.maxLength = 51
      schema.$defs.ChangeWatchId.pattern = pattern
    })
    const result = generate(path, "generate-product-protocol")
    assert.equal(result.status, 0, result.stderr)
    const output = (name) => readFileSync(join(path, name), "utf8")
    const ts = output("packages/nessa-client/src/generated/product.ts")
    const rust = output("crates/nessa-protocol/src/product/generated.rs")
    const outcomes = output("crates/nessa-protocol/src/product_contract/generated.rs")
    assert.match(ts, /maxChangeWatchIdBytes = 51/)
    assert.ok(ts.includes(`changeWatchIdPattern = ${JSON.stringify(pattern)}`))
    assert.match(ts, /globalOwners: 17/)
    assert.match(ts, /principalOwners: 5/)
    assert.match(ts, /recordTargets: 3/)
    assert.match(ts, /catalogueTargets: 2/)
    assert.match(rust, /MAX_CHANGE_WATCH_ID_BYTES: usize = 51;/)
    assert.ok(
      rust.includes(`CHANGE_WATCH_ID_PATTERN: &str = ${JSON.stringify(pattern)};`),
    )
    assert.match(rust, /MAX_GLOBAL_CHANGE_WATCHES: usize = 17;/)
    assert.match(rust, /MAX_PRINCIPAL_CHANGE_WATCHES: usize = 5;/)
    assert.match(rust, /MAX_CONNECTION_RECORD_WATCHES: usize = 3;/)
    assert.match(rust, /MAX_CONNECTION_CATALOGUE_WATCHES: usize = 2;/)
    // Per-connection capacity is derived, never a second number to keep in step.
    assert.match(rust, /MAX_CONNECTION_CHANGE_WATCHES: usize = 5;/)
    assert.match(ts, /export type ChangeWatchId = string/)
    assert.match(rust, /pub type ChangeWatchId = String;/)
    assert.match(rust, /pub reason: ChangeWatchEndReason/)
    assert.match(outcomes, /pub enum ChangeWatchEndReason/)
    assert.doesNotMatch(outcomes, /impl ChangeWatchEndReason/)
    assert.match(outcomes, /impl ChangeWatchErrorCode/)
  }))

for (const [name, value] of [
  ["globalOwners", undefined],
  ["globalOwners", 0],
  ["globalOwners", "64"],
  ["principalOwners", 65],
  ["recordTargets", -1],
  ["catalogueTargets", 1.5],
  ["connectionTargets", 2],
  ["unknownOwners", 1],
])
  test(`invalid watch ${name} ${value} preserves every unpublished artifact`, () =>
    fixture((path) => {
      edit(path, "protocol/product/v1.json", (schema) => {
        schema["x-changeWatchLimits"][name] = value
      })
      const result = unchanged(
        path,
        ["protocol/product/v1.json", ...productOutputs],
        () => generate(path, "generate-product-protocol"),
      )
      assert.match(result.stderr, /Invalid change watch limit/)
    }))
