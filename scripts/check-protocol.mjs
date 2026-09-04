#!/usr/bin/env node
/**
 * Validate protocol SSOT: manifest refs, schema files, fixtures, generated types.
 */
import Ajv2020 from "ajv/dist/2020.js"
import addFormats from "ajv-formats"
import { execSync } from "node:child_process"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const protocolDir = join(root, "protocol")
const failures = []

function rel(path) {
  return relative(root, path)
}

function fail(file, rule) {
  failures.push(`${rel(file)}: ${rule}`)
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"))
}

const manifestPath = join(protocolDir, "manifest.json")
if (!existsSync(manifestPath)) {
  fail(manifestPath, "protocol manifest missing")
  process.exit(1)
}

const manifest = readJson(manifestPath)
const ajv = new Ajv2020({ allErrors: true, strict: false, validateSchema: false })
addFormats(ajv)

const schemaDir = join(protocolDir, "schemas/v1")
const loadOrder = [
  "common.json",
  "frames.json",
  "shortcuts.json",
  "connect.json",
  "server.json",
  "export.json",
]
for (const name of loadOrder) {
  const path = join(schemaDir, name)
  if (!existsSync(path)) continue
  try {
    ajv.addSchema(readJson(path))
  } catch (error) {
    fail(path, `invalid schema JSON: ${error.message}`)
  }
}

for (const [name, path] of Object.entries(manifest.schemas ?? {})) {
  const full = join(protocolDir, path)
  if (!existsSync(full)) fail(full, `manifest.schemas.${name} missing`)
}

for (const [method, spec] of Object.entries(manifest.methods ?? {})) {
  for (const slot of ["params", "result"]) {
    const ref = spec[slot]
    if (ref === null) continue
    if (!ref) {
      fail(manifestPath, `method ${method} missing ${slot} ref`)
      continue
    }
    const file = join(protocolDir, ref.split("#")[0])
    if (!existsSync(file))
      fail(manifestPath, `method ${method} ${slot} ref not found: ${ref}`)
  }
}

for (const [event, spec] of Object.entries(manifest.events ?? {})) {
  const ref = spec.payload
  const file = join(protocolDir, ref.split("#")[0])
  if (!existsSync(file))
    fail(manifestPath, `event ${event} payload ref not found: ${ref}`)
}

const fixtureDir = join(protocolDir, "fixtures/v1")
const fixtureSpecs = [
  {
    file: "connect-req.json",
    validate: (doc) => {
      const schema = ajv.getSchema("nessa://protocol/v1/frames.json#/$defs/ReqFrame")
      if (!schema?.(doc)) return schema?.errors
      const params = ajv.getSchema(
        "nessa://protocol/v1/connect.json#/$defs/ConnectParams",
      )
      return params?.(doc.params) ? null : params?.errors
    },
  },
  {
    file: "connect-res.json",
    validate: (doc) => {
      const frame = ajv.getSchema("nessa://protocol/v1/frames.json#/$defs/ResFrame")
      if (!frame?.(doc)) return frame?.errors
      const payload = ajv.getSchema("nessa://protocol/v1/connect.json#/$defs/HelloOk")
      return payload?.(doc.payload) ? null : payload?.errors
    },
  },
  {
    file: "connect-challenge.json",
    validate: (doc) => {
      const frame = ajv.getSchema("nessa://protocol/v1/frames.json#/$defs/EventFrame")
      if (!frame?.(doc)) return frame?.errors
      const payload = ajv.getSchema(
        "nessa://protocol/v1/connect.json#/$defs/ConnectChallenge",
      )
      return payload?.(doc.payload) ? null : payload?.errors
    },
  },
  {
    file: "server-health-res.json",
    validate: (doc) => {
      const frame = ajv.getSchema("nessa://protocol/v1/frames.json#/$defs/ResFrame")
      if (!frame?.(doc)) return frame?.errors
      const payload = ajv.getSchema("nessa://protocol/v1/server.json#/$defs/HealthResult")
      return payload?.(doc.payload) ? null : payload?.errors
    },
  },
]

for (const { file, validate } of fixtureSpecs) {
  const path = join(fixtureDir, file)
  if (!existsSync(path)) {
    fail(path, "fixture missing")
    continue
  }
  const doc = readJson(path)
  const errors = validate(doc)
  if (errors) fail(path, `fixture failed validation: ${JSON.stringify(errors)}`)
}

const generatedFiles = [
  {
    path: join(root, "packages/nessa-client/src/generated/protocol.ts"),
    envKey: "NESSA_PROTOCOL_OUT",
    label: "generated types",
  },
  {
    path: join(root, "packages/nessa-client/src/generated/catalog.ts"),
    envKey: "NESSA_PROTOCOL_CATALOG_TS_OUT",
    label: "generated method/event catalog (TS)",
  },
  {
    path: join(root, "crates/nessa-server/src/protocol/generated_catalog.rs"),
    envKey: "NESSA_PROTOCOL_CATALOG_RS_OUT",
    label: "generated method/event catalog (Rust)",
  },
  {
    path: join(root, "crates/nessa-server/src/protocol/generated_types.rs"),
    envKey: "NESSA_PROTOCOL_TYPES_RS_OUT",
    label: "generated payload types (Rust)",
  },
]

const tempDir = mkdtempSync(join(tmpdir(), "nessa-protocol-check-"))
try {
  const env = {
    ...process.env,
    NESSA_PROTOCOL_OUT: join(tempDir, "protocol.ts"),
    NESSA_PROTOCOL_CATALOG_TS_OUT: join(tempDir, "catalog.ts"),
    NESSA_PROTOCOL_CATALOG_RS_OUT: join(tempDir, "generated_catalog.rs"),
    NESSA_PROTOCOL_TYPES_RS_OUT: join(tempDir, "generated_types.rs"),
  }
  execSync("node scripts/generate-protocol-types.mjs", {
    cwd: root,
    stdio: "pipe",
    env,
  })

  for (const file of generatedFiles) {
    if (!existsSync(file.path)) {
      fail(file.path, `${file.label} missing — run pnpm protocol:generate`)
      continue
    }
    const before = readFileSync(file.path, "utf8")
    const expected = readFileSync(env[file.envKey], "utf8")
    if (before !== expected) {
      fail(file.path, `${file.label} stale — run pnpm protocol:generate and commit`)
    }
  }
} finally {
  rmSync(tempDir, { recursive: true, force: true })
}

if (failures.length > 0) {
  console.error("protocol check failed:\n")
  for (const line of failures) console.error(`  ${line}`)
  process.exit(1)
}

console.log("protocol check passed")
