import { existsSync, lstatSync, readdirSync } from "node:fs"
import { dirname, join, resolve } from "node:path"

export function normalizedPath(path) {
  return path.replaceAll("\\", "/")
}

/** Replace Rust comments and string contents with spaces while preserving lines. */
export function maskRustNonCode(source) {
  const masked = [...source]
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
      if ([".git", "node_modules", "target"].includes(name)) continue
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

export function rustBoundaryViolations(path, source) {
  path = normalizedPath(path)
  const domain = /(?:\/domain\/|\/domain\.rs$)/.test(path)
  const application = /(?:\/application\/|\/application\.rs$)/.test(path)
  if (!domain && !application) return []
  const imports = [...maskRustNonCode(source).matchAll(/\buse\s+([^;]+);/g)].map(
    (match) => match[1],
  )
  const failures = new Set()
  for (const item of imports) {
    if (/\b(infrastructure|composition|product|adapters|presentation)\b/.test(item))
      failures.add(
        "domain/application imports must point inward, through application-owned ports",
      )
    if (domain && /\bapplication\b/.test(item))
      failures.add("domain must not import application DTOs or orchestration")
    if (domain && /\b(tokio|tauri|serde|serde_json|reqwest|axum|shepherd)\b/.test(item))
      failures.add(
        "domain must not import runtime, transport, or serialization libraries",
      )
    if (domain && /\bstd\b[\s\S]*\b(fs|net|process)\b/.test(item))
      failures.add("domain must not import filesystem, network, or process effects")
  }
  return [...failures]
}
