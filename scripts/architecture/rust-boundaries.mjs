import { existsSync, lstatSync, readdirSync } from "node:fs"
import { dirname, join, resolve } from "node:path"

/** Normalize platform path separators for layer classification. */
export function normalizedPath(path) {
  return path.replaceAll("\\", "/")
}

/** Replace Rust comments and string contents with spaces while preserving lines. */
export function maskRustNonCode(source) {
  const masked = source.split("")
  const blank = (index) => {
    if (masked[index] !== "\n" && masked[index] !== "\r") masked[index] = " "
  }
  for (let index = 0; index < source.length;) {
    if (source.startsWith("//", index)) {
      while (index < source.length && source[index] !== "\n") blank(index++)
      continue
    }
    if (source.startsWith("/*", index)) {
      let depth = 0
      while (index < source.length) {
        if (source.startsWith("/*", index)) {
          blank(index++)
          blank(index++)
          depth += 1
        } else if (source.startsWith("*/", index)) {
          blank(index++)
          blank(index++)
          depth -= 1
          if (depth === 0) break
        } else blank(index++)
      }
      continue
    }
    const character = source
      .slice(index)
      .match(/^(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|[\s\S])|[^'\\\r\n])'/u)
    if (character) {
      for (let count = 0; count < character[0].length; count += 1) blank(index++)
      continue
    }
    const raw = source.slice(index).match(/^(?:br|rb|r)(#*)"/)
    if (raw) {
      const closing = `"${raw[1]}`
      for (let count = 0; count < raw[0].length; count += 1) blank(index++)
      while (index < source.length && !source.startsWith(closing, index)) blank(index++)
      for (let count = 0; count < closing.length && index < source.length; count += 1)
        blank(index++)
      continue
    }
    const prefix = source[index] === "b" && source[index + 1] === '"' ? 1 : 0
    if (source[index + prefix] === '"') {
      if (prefix) blank(index++)
      blank(index++)
      while (index < source.length) {
        if (source[index] === "\\") {
          blank(index++)
          if (index < source.length) blank(index++)
        } else if (source[index] === '"') {
          blank(index++)
          break
        } else blank(index++)
      }
      continue
    }
    index += 1
  }
  return masked.join("")
}

function findTauriRoots(root) {
  const found = []
  const visit = (directory) => {
    for (const name of readdirSync(directory)) {
      // Local agent worktrees are other checkouts, not this workspace's source.
      if ([".git", ".claude", "node_modules", "target"].includes(name)) continue
      const path = join(directory, name)
      const entry = lstatSync(path)
      if (entry.isSymbolicLink() || !entry.isDirectory()) continue
      if (name === "src-tauri" && existsSync(join(path, "Cargo.toml"))) found.push(path)
      else visit(path)
    }
  }
  visit(root)
  return found
}

/** Return every workspace package root plus every standalone Tauri Rust root. */
export function workspaceRustSourceRoots(root, metadata) {
  const workspace = new Set(metadata.workspace_members)
  const roots = metadata.packages
    .filter((pkg) => workspace.has(pkg.id))
    .map((pkg) => dirname(pkg.manifest_path))
  for (const tauri of findTauriRoots(root)) roots.push(tauri)
  return [...new Set(roots.map((path) => resolve(path)))]
}

// These std values represent addresses; none can open or operate a socket.
function pureNetValueReference(item) {
  const match =
    /^std::net::(?:([A-Za-z0-9_]+)(?:::[A-Za-z0-9_]+)*|\{([\s\S]*)\})(?:\s+as\s+[A-Za-z0-9_]+)?$/.exec(
      item,
    )
  if (!match) return false
  const names = (match[1] ?? match[2])
    .split(",")
    .map((name) => name.trim().split(/\s+as\s+/)[0])
    .filter(Boolean)
  const values = new Set([
    "IpAddr",
    "Ipv4Addr",
    "Ipv6Addr",
    "SocketAddr",
    "SocketAddrV4",
    "SocketAddrV6",
  ])
  return names.length > 0 && names.every((name) => values.has(name))
}

/** Check written domain/application dependencies, without resolving Rust names. */
export function rustBoundaryViolations(path, source) {
  path = normalizedPath(path)
  const domain = /(?:\/domain\/|\/domain\.rs$)/.test(path)
  const application = /(?:\/application\/|\/application\.rs$)/.test(path)
  if (!domain && !application) return []
  const code = maskRustNonCode(source)
  const imports = [...code.matchAll(/\buse\s+([^;]+);/g)].map((match) => match[1])
  // Imports retain their grouped tree for the existing policy. Outside them,
  // explicit paths name dependencies in expressions, types and attributes.
  // This reads written paths, not names resolved through re-exports or macros.
  const qualified = [
    ...code
      .replace(/\buse\s+[^;]+;/g, " ")
      .matchAll(
        /(?<![\p{ID_Continue}])(?:r#)?[\p{ID_Start}_][\p{ID_Continue}]*(?:\s*::\s*(?:r#)?[\p{ID_Start}_][\p{ID_Continue}]*)+/gu,
      ),
  ].map((match) => match[0])
  const references = [
    ...imports.map((item) => ({ item, isImport: true })),
    ...qualified.map((item) => ({ item, isImport: false })),
  ].map(({ item, isImport }) => ({
    item: item
      .trim()
      .replace(/\s*::\s*/g, "::")
      .replace(/\br#/g, "")
      .replace(/^::/, ""),
    isImport,
  }))
  const failures = new Set()
  for (const { item, isImport } of references) {
    // An expression's final segment selects an item, not its dependency:
    // Order::product() is a method call. Imports also admit module names.
    const dependency = isImport ? item : item.slice(0, item.lastIndexOf("::"))
    const names = new Set(dependency.match(/[\p{ID_Start}_][\p{ID_Continue}]*/gu))
    if (
      ["infrastructure", "composition", "product", "adapters", "presentation"].some(
        (name) => names.has(name),
      )
    )
      failures.add(
        "domain/application references must point inward, through application-owned ports",
      )
    if (domain && names.has("application"))
      failures.add("domain must not reference application DTOs or orchestration")
    if (
      domain &&
      ["tokio", "tauri", "serde", "serde_json", "reqwest", "axum", "shepherd"].some(
        (name) => names.has(name),
      )
    )
      failures.add(
        "domain must not reference runtime, transport, or serialization libraries",
      )
    // The sync engine's own `replication::domain` values (`Id`, `Scope`) are
    // pure; its catalogue, transport and source modules are not.
    if (
      domain &&
      names.has("nessa_sync") &&
      item !== "nessa_sync::replication::domain" &&
      !item.startsWith("nessa_sync::replication::domain::")
    )
      failures.add(
        "domain must not reference the sync engine beyond its replication::domain values",
      )
    if (
      domain &&
      names.has("std") &&
      ["fs", "net", "process"].some((name) => names.has(name)) &&
      !pureNetValueReference(item)
    )
      failures.add("domain must not reference filesystem, network, or process effects")
  }
  return [...failures]
}
