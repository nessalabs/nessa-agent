/** Generate the separate product profile from its JSON Schema. --check detects drift. */
import { readFileSync, writeFileSync, mkdirSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { format, resolveConfig } from "prettier"
import { spawnSync } from "node:child_process"
import { validateExternalRustTypes } from "./product-protocol/rust-types.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const schema = JSON.parse(readFileSync(resolve(root, "protocol/product/v1.json"), "utf8"))
const manifest = JSON.parse(
  readFileSync(resolve(root, "protocol/product/manifest.json"), "utf8"),
)
const snake = (name) => name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)
validateExternalRustTypes(schema.$defs)
const externalRustTypes = new Set()
const rustTypeName = (value) => value.split("::").at(-1)
function type(node, rust) {
  if (node.$ref) {
    const name = node.$ref.split("/").at(-1)
    const externalType = schema.$defs[name]["x-rust-type"]
    if (rust && externalType) externalRustTypes.add(externalType)
    return rust ? (externalType ? rustTypeName(externalType) : name) : name
  }
  if (node.enum && !rust) return node.enum.map(JSON.stringify).join(" | ")
  if (Array.isArray(node.type) && rust)
    return `Option<${type({ ...node, type: node.type.find((value) => value !== "null") }, true)}>`
  if (Array.isArray(node.type) && !rust)
    return node.type
      .map((value) => (value === "null" ? "null" : type({ ...node, type: value }, false)))
      .join(" | ")
  if (node.const !== undefined && !rust) return JSON.stringify(node.const)
  if (node.type === "string") return rust ? "String" : "string"
  if (node.type === "integer") return rust ? "u64" : "number"
  if (node.type === "boolean") return rust ? "bool" : "boolean"
  if (node.type === "array")
    return rust ? `Vec<${type(node.items, true)}>` : `${type(node.items, false)}[]`
  throw new Error(`Unsupported schema node: ${JSON.stringify(node)}`)
}
// Keep schema documentation in generated declarations so TypeDoc can render it.
function doc(description) {
  return description ? `/** ${description.replaceAll("*/", "* /")} */\n` : ""
}
let ts =
  "/* eslint-disable */\n/* Generated from protocol/product/v1.json and manifest.json. Do not edit. */\n"
let rs =
  "//! Generated from protocol/product/v1.json. Do not edit.\n//! Bounds are validated at the transport boundary; these are payload types only.\n#![allow(dead_code)]\nuse serde::{Deserialize, Serialize};\n"
rs += "__EXTERNAL_RUST_IMPORTS__"
for (const [name, def] of Object.entries(schema.$defs)) {
  ts += doc(def.description)
  if (def.enum) {
    const pascal = (value) =>
      value
        .split("_")
        .map((p) => p[0].toUpperCase() + p.slice(1))
        .join("")
    ts += `export const ${name} = ${JSON.stringify(Object.fromEntries(def.enum.map((v) => [pascal(v), v])))} as const\nexport type ${name} = typeof ${name}[keyof typeof ${name}]\n`
    rs += `#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]\n#[serde(rename_all = "snake_case")]\npub enum ${name} {${def.enum.map(pascal).join(",")}}\n`
    // The wire spelling, so handlers pass the typed value where a code is written.
    rs += `impl ${name} { pub fn as_str(self) -> &'static str { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${JSON.stringify(v)}`).join(",")} } } }\n`
    if (def["x-close-policy"]) {
      ts += `export const sessionClosePolicy = ${JSON.stringify(def["x-close-policy"])} as const\n`
      rs += `impl ${name} { pub fn web_socket_code(self) -> u16 { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].webSocketCode}`).join(",")} } } pub fn retryable(self) -> bool { match self {${def.enum.map((v) => `Self::${pascal(v)} => ${def["x-close-policy"][v].retryable}`).join(",")} } } }\n`
    }
    continue
  }
  ts += `export interface ${name} {\n`
  const externalRust = Boolean(def["x-rust-type"])
  // No Debug or Clone: some payloads contain one-time credentials.
  if (!externalRust)
    rs += `#[derive(Deserialize, Serialize)]\n#[serde(rename_all = "camelCase", deny_unknown_fields)]\npub struct ${name} {\n`
  for (const [field, node] of Object.entries(def.properties)) {
    const optional = !def.required.includes(field)
    ts += doc(node.description)
    ts += `  ${field}${optional ? "?" : ""}: ${type(node, false)}\n`
    if (!externalRust)
      rs += `${optional ? '#[serde(default, skip_serializing_if = "Option::is_none")]\n' : ""}pub ${snake(field)}: ${optional ? `Option<${type(node, true).replace(/^Option<(.*)>$/, "$1")}>` : type(node, true)},\n`
  }
  ts += "}\n"
  if (!externalRust) rs += "}\n"
}
for (const [kind, entries] of [
  ["Method", manifest.methods],
  ["Event", manifest.events],
]) {
  const key = (name) =>
    name
      .split(".")
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join("")
  ts += `export const Product${kind} = ${JSON.stringify(Object.fromEntries(Object.keys(entries).map((name) => [key(name), name])))} as const\n`
}
ts = await format(ts, {
  ...(await resolveConfig(resolve(root, "prettier.config.js"))),
  parser: "typescript",
})
let imports = ""
const rustImports = new Map()
for (const externalType of externalRustTypes) {
  const separator = externalType.lastIndexOf("::")
  const module = externalType.slice(0, separator)
  const name = externalType.slice(separator + 2)
  const names = rustImports.get(module) ?? []
  names.push(name)
  rustImports.set(module, names)
}
for (const [module, names] of rustImports) {
  imports += `use ${module}::{${names.sort().join(", ")}};\n`
}
rs = rs.replace("__EXTERNAL_RUST_IMPORTS__", imports)
const formatted = spawnSync("rustfmt", ["--edition", "2021"], {
  input: rs,
  encoding: "utf8",
})
if (formatted.status !== 0) throw new Error(formatted.stderr)
for (const [path, contents] of [
  ["packages/nessa-client/src/generated/product.ts", ts],
  ["crates/nessa-server/src/product/generated.rs", formatted.stdout],
]) {
  const target = resolve(root, path)
  if (process.argv.includes("--check")) {
    if (readFileSync(target, "utf8") !== contents)
      throw new Error(`Generated product protocol is stale: ${path}`)
  } else {
    mkdirSync(dirname(target), { recursive: true })
    writeFileSync(target, contents)
  }
}
