#!/usr/bin/env node
/**
 * Generate protocol artifacts from protocol/ SSOT.
 *
 * SSOT:
 *   protocol/manifest.json     → method/event names
 *   protocol/schemas/v1/*.json → payload shapes
 *
 * Outputs (do not edit by hand):
 *   packages/nessa-client/src/generated/protocol.ts
 *   packages/nessa-client/src/generated/catalog.ts
 *   crates/nessa-server/src/protocol/generated_catalog.rs
 *   crates/nessa-server/src/protocol/generated_types.rs
 */
import ts from "typescript"
import { compile } from "json-schema-to-typescript"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { format, resolveConfig } from "prettier"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const protocolDir = join(root, "protocol")
const schemaDir = join(protocolDir, "schemas/v1")
const exportSchema = JSON.parse(readFileSync(join(schemaDir, "export.json"), "utf8"))
const manifest = JSON.parse(readFileSync(join(protocolDir, "manifest.json"), "utf8"))

const outDir = join(root, "packages/nessa-client/src/generated")
const outFile = process.env.NESSA_PROTOCOL_OUT ?? join(outDir, "protocol.ts")
const catalogTsOut =
  process.env.NESSA_PROTOCOL_CATALOG_TS_OUT ?? join(outDir, "catalog.ts")
const catalogRsOut =
  process.env.NESSA_PROTOCOL_CATALOG_RS_OUT ??
  join(root, "crates/nessa-server/src/protocol/generated_catalog.rs")
const typesRsOut =
  process.env.NESSA_PROTOCOL_TYPES_RS_OUT ??
  join(root, "crates/nessa-server/src/protocol/generated_types.rs")

const SCHEMA_FILES = ["common.json", "server.json", "shortcuts.json", "conversation.json"]

function constName(wireName) {
  return wireName
    .split(/[.\-/]/)
    .filter(Boolean)
    .map((part) => part.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toUpperCase())
    .join("_")
}

function tsKey(wireName) {
  return wireName
    .split(/[.\-/]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join("")
}

function toPascal(name) {
  return name.charAt(0).toUpperCase() + name.slice(1)
}

function toSnake(name) {
  return name
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/\./g, "_")
    .toLowerCase()
}

function enumVariantName(wireValue) {
  if (wireValue === "*") return "Any"
  return wireValue
    .split(/[.\-/]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join("")
}

function resolveRef(ref, schemasByFile, currentFile) {
  const [pathPart, fragment] = ref.split("#")
  const file = pathPart ? pathPart.replace(/^\.\//, "") : currentFile
  const schema = schemasByFile[file]
  if (!schema) throw new Error(`unknown schema ref file: ${ref}`)
  const defName = fragment?.replace(/^\/\$defs\//, "")
  if (!defName || !schema.$defs?.[defName]) {
    throw new Error(`unknown schema ref: ${ref}`)
  }
  return { file, name: defName, schema: schema.$defs[defName] }
}

function rustTypeForSchema(schema, ctx) {
  if (schema.$ref) {
    const resolved = resolveRef(schema.$ref, ctx.schemasByFile, ctx.currentFile)
    return resolved.name
  }
  if (schema.enum) {
    throw new Error("inline enums are not supported; use $ref to a named $defs enum")
  }
  if (schema.type === "string") return "String"
  if (schema.type === "boolean") return "bool"
  if (schema.type === "integer") {
    return "i64"
  }
  if (schema.type === "array") {
    const item = rustTypeForSchema(schema.items, ctx)
    return `Vec<${item}>`
  }
  if (schema.type === "object" || schema.properties) {
    if (!schema.properties || Object.keys(schema.properties).length === 0) {
      if (schema.additionalProperties === false) return null
      return "serde_json::Map<String, serde_json::Value>"
    }
    throw new Error("inline objects must be named via title or extracted first")
  }
  if (
    !schema.type &&
    Object.keys(schema).every((key) => ["description", "tsType"].includes(key))
  ) {
    return "serde_json::Value"
  }
  throw new Error(`unsupported schema node: ${JSON.stringify(schema)}`)
}

function collectInlineObjects(defName, schema, out, ctx) {
  if (!schema.properties) return
  for (const [prop, propSchema] of Object.entries(schema.properties)) {
    if (propSchema.$ref || propSchema.enum || propSchema.type !== "object") continue
    if (!propSchema.properties) continue
    const nestedName = propSchema.title || `${defName}${toPascal(prop)}`
    if (!out.has(nestedName)) {
      out.set(nestedName, { schema: propSchema, file: ctx.currentFile })
      collectInlineObjects(nestedName, propSchema, out, ctx)
    }
    propSchema.__rustTypeName = nestedName
  }
}

function emitEnum(name, schema) {
  const variants = schema.enum.map((value) => {
    const variant = enumVariantName(value)
    return `    #[serde(rename = "${value}")]\n    ${variant},`
  })
  return [
    "#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]",
    `pub enum ${name} {`,
    ...variants,
    "}",
    "",
  ].join("\n")
}

function emitStruct(name, schema, ctx) {
  const required = new Set(schema.required ?? [])
  const fields = []
  for (const [prop, propSchema] of Object.entries(schema.properties ?? {})) {
    let ty
    if (propSchema.__rustTypeName) {
      ty = propSchema.__rustTypeName
    } else if (
      propSchema.$ref ||
      propSchema.type ||
      Object.keys(propSchema).every((key) => ["description", "tsType"].includes(key))
    ) {
      ty = rustTypeForSchema(propSchema, ctx)
    } else {
      throw new Error(`cannot type field ${name}.${prop}`)
    }
    if (ty === null) continue
    const snake = toSnake(prop)
    const optional = !required.has(prop)
    const rustTy = optional ? `Option<${ty}>` : ty
    const attrs = []
    if (optional)
      attrs.push('    #[serde(default, skip_serializing_if = "Option::is_none")]')
    if (propSchema.description) {
      attrs.push(`    /// ${propSchema.description.replace(/\n/g, " ")}`)
    }
    fields.push(
      `${attrs.join("\n")}${attrs.length ? "\n" : ""}    pub ${snake}: ${rustTy},`,
    )
  }

  const deny = schema.additionalProperties === false ? ", deny_unknown_fields" : ""
  return [
    "#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]",
    `#[serde(rename_all = "camelCase"${deny})]`,
    `pub struct ${name} {`,
    ...fields,
    "}",
    "",
  ].join("\n")
}

function generateRustTypes(schemasByFile) {
  const named = new Map()

  for (const file of SCHEMA_FILES) {
    const schema = schemasByFile[file]
    for (const [name, def] of Object.entries(schema.$defs ?? {})) {
      named.set(name, { schema: def, file })
      collectInlineObjects(name, def, named, {
        schemasByFile,
        currentFile: file,
      })
    }
  }

  // Empty params object for methods with params: null in the manifest.
  named.set("HealthParams", {
    schema: {
      type: "object",
      additionalProperties: false,
      properties: {},
    },
    file: "server.json",
  })

  const lines = [
    "//! Generated from `protocol/schemas/v1/*.json` — do not edit by hand.",
    "//!",
    "//! **Source of truth:** `protocol/schemas/v1/` (payload shapes).",
    "//! Regenerate: `pnpm protocol:generate`",
    "//!",
    "//! Envelope helpers (`RequestFrame` encode/decode) stay hand-written in",
    "//! `frames.rs`; this file is the payload/type catalog only.",
    "",
    "#![allow(dead_code)]",
    "",
    "use serde::{Deserialize, Serialize};",
    "",
  ]

  // Enums first, then structs (stable order).
  const entries = [...named.entries()].sort(([a], [b]) => a.localeCompare(b))
  for (const [name, { schema, file }] of entries) {
    const ctx = { schemasByFile, currentFile: file }
    if (schema.enum) {
      lines.push(emitEnum(name, schema))
      continue
    }
    if (schema.type === "object" || schema.properties) {
      if (!schema.properties || Object.keys(schema.properties).length === 0) {
        lines.push(
          "#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]",
          `pub struct ${name} {}`,
          "",
        )
        continue
      }
      lines.push(emitStruct(name, schema, ctx))
      continue
    }
    throw new Error(`unsupported top-level def ${name}`)
  }

  return lines.join("\n")
}

mkdirSync(dirname(outFile), { recursive: true })
mkdirSync(dirname(catalogTsOut), { recursive: true })
mkdirSync(dirname(catalogRsOut), { recursive: true })
mkdirSync(dirname(typesRsOut), { recursive: true })

const tsBanner =
  "/* eslint-disable */\n" +
  "/**\n" +
  " * Generated from protocol/schemas/v1/*.json — do not edit by hand.\n" +
  " *\n" +
  " * **Source of truth:** `protocol/schemas/v1/` (payload shapes).\n" +
  " * Method/event *names* → see `./catalog.ts` (from `protocol/manifest.json`).\n" +
  " * Regenerate: `pnpm protocol:generate`\n" +
  " */\n\n"

const chunks = [tsBanner]

for (const [name, ref] of Object.entries(exportSchema.$defs)) {
  const wrapper = {
    $schema: exportSchema.$schema,
    title: name,
    $ref: ref.$ref ?? ref,
  }
  const ts = await compile(wrapper, name, {
    cwd: schemaDir,
    bannerComment: "",
    additionalProperties: false,
    enableConstEnums: true,
    strictIndexSignatures: true,
  })
  chunks.push(ts.trim(), "\n\n")
}

// json-schema-to-typescript inlines $refs per compile, so shared defs like
// SurfaceInfo / ClientInfo / GatewayError appear once per referencing type.
// Keep the first declaration of each exported name.
const deduped = restoreSchemaDocs(dedupeNamedExports(chunks.join("")))
const prettierConfig = (await resolveConfig(join(root, "prettier.config.js"))) ?? {}
const enumConstants = SCHEMA_FILES.flatMap((file) =>
  Object.entries(JSON.parse(readFileSync(join(schemaDir, file), "utf8")).$defs ?? {})
    .filter(([, def]) => def.enum)
    .map(
      ([name, def]) =>
        `export const ${name} = ${JSON.stringify(Object.fromEntries(def.enum.map((value) => [enumVariantName(value), value])))} as const`,
    ),
).join("\n")
const formatted = await format(deduped + "\n" + enumConstants, {
  ...prettierConfig,
  filepath: outFile,
})
writeFileSync(outFile, formatted)
console.log(`wrote ${outFile}`)

/** $ref compilation can move property descriptions onto referenced declarations.
 * Restore canonical declaration and field comments from their owning schemas.
 */
function restoreSchemaDocs(source) {
  const definitions = Object.assign(
    {},
    ...[...SCHEMA_FILES, "frames.json"].map(
      (file) => JSON.parse(readFileSync(join(schemaDir, file), "utf8")).$defs,
    ),
  )
  const tree = ts.createSourceFile("protocol.ts", source, ts.ScriptTarget.Latest, true)
  const edits = []
  function document(node, description) {
    if (!description) return
    const start = node.jsDoc?.[0]?.pos ?? node.getStart(tree)
    const end = node.jsDoc?.at(-1)?.end ?? start
    edits.push({ start, end, text: `/** ${description.replaceAll("*/", "* /")} */\n` })
  }
  for (const node of tree.statements) {
    if (!ts.isInterfaceDeclaration(node) && !ts.isTypeAliasDeclaration(node)) continue
    const definition = definitions[node.name.text]
    if (!definition) continue
    document(node, definition.description)
    if (ts.isInterfaceDeclaration(node)) {
      for (const member of node.members)
        document(member, definition.properties?.[member.name?.getText(tree)]?.description)
    }
  }
  for (const edit of edits.sort((a, b) => b.start - a.start)) {
    source = source.slice(0, edit.start) + edit.text + source.slice(edit.end)
  }
  return source
}

/** Keep the first `export interface|type Name` block for each Name. */
function dedupeNamedExports(source) {
  const starts = [...source.matchAll(/^export (?:interface|type) (\w+)\b/gm)].map(
    (match) => {
      const prefix = source.slice(0, match.index).trimEnd()
      // A declaration's leading JSDoc belongs to it, not the preceding export.
      const start = prefix.endsWith("*/") ? prefix.lastIndexOf("/**") : match.index
      return { name: match[1], index: start >= 0 ? start : match.index }
    },
  )
  if (!starts.length) return source
  const seen = new Set()
  const kept = []
  for (let i = 0; i < starts.length; i++) {
    const { name, index } = starts[i]
    if (seen.has(name)) continue
    seen.add(name)
    kept.push(source.slice(index, starts[i + 1]?.index ?? source.length).trimEnd())
  }
  return `${source.slice(0, starts[0].index)}${kept.join("\n\n")}\n`
}

const methodEntries = Object.keys(manifest.methods ?? {}).sort()
const eventEntries = Object.keys(manifest.events ?? {}).sort()

const catalogTs = `/* eslint-disable */
/**
 * Generated from protocol/manifest.json — do not edit by hand.
 *
 * **Source of truth:** \`protocol/manifest.json\` (method/event names).
 * Payload shapes → see \`./protocol.ts\` (from \`protocol/schemas/v1/\`).
 * Regenerate: \`pnpm protocol:generate\`
 *
 * To add a method: edit manifest + schemas, run \`pnpm protocol:generate\`,
 * wire the handler — do not invent string literals in client/server code.
 */

export const Method = {
${methodEntries.map((name) => `  ${tsKey(name)}: "${name}",`).join("\n")}
} as const

export type MethodName = (typeof Method)[keyof typeof Method]

export const Event = {
${eventEntries.map((name) => `  ${tsKey(name)}: "${name}",`).join("\n")}
} as const

export type EventName = (typeof Event)[keyof typeof Event]
`

writeFileSync(
  catalogTsOut,
  catalogTs.replace("export const Event = {\n\n}", "export const Event = {}"),
)
console.log(`wrote ${catalogTsOut}`)

const catalogRs = `//! Generated from \`protocol/manifest.json\` — do not edit by hand.
//!
//! **Source of truth:** \`protocol/manifest.json\` (method/event names).
//! Payload structs → \`generated_types.rs\` (from \`protocol/schemas/v1/\`).
//! Regenerate: \`pnpm protocol:generate\`
//!
//! To add a method: edit manifest + schemas, run \`pnpm protocol:generate\`,
//! wire the handler — do not invent string literals in client/server code.

#![allow(dead_code)]

/// RPC method names on \`type: "req"\` frames.
pub mod method {
${methodEntries
  .map((name) => `    pub const ${constName(name)}: &str = "${name}";`)
  .join("\n")}
}

/// Server push event names on \`type: "event"\` frames.
pub mod event {
${eventEntries
  .map((name) => `    pub const ${constName(name)}: &str = "${name}";`)
  .join("\n")}
}
`

writeFileSync(catalogRsOut, catalogRs.replace("pub mod event {\n\n}", "pub mod event {}"))
console.log(`wrote ${catalogRsOut}`)

const schemasByFile = Object.fromEntries(
  SCHEMA_FILES.map((file) => [
    file,
    JSON.parse(readFileSync(join(schemaDir, file), "utf8")),
  ]),
)
const typesRs = generateRustTypes(schemasByFile)
writeFileSync(typesRsOut, typesRs)
console.log(`wrote ${typesRsOut}`)
