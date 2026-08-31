#!/usr/bin/env node
/**
 * Contract checks for docs/codebase-structure.md and docs/ARCHITECTURE.md.
 * Failures are the rule plus the file that broke it — not a style opinion.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const src = join(root, "src")
const failures = []

function walk(dir) {
  const files = []
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) files.push(...walk(path))
    else if (/\.(ts|tsx|js|mjs)$/.test(name)) files.push(path)
  }
  return files
}

function rel(path) {
  return relative(root, path)
}

function fail(path, rule) {
  failures.push(`${rel(path)}: ${rule}`)
}

if (existsSync(join(src, "utils.ts")) || existsSync(join(src, "utils.tsx"))) {
  fail(join(src, "utils.ts"), "there is no utils module")
}

const draftReplyHome = "src/conversation/application/internal/stand-in.ts"

for (const file of walk(src)) {
  const text = readFileSync(file, "utf8")
  const path = rel(file)

  const inConversationRules =
    path.startsWith("src/conversation/model/") ||
    path.startsWith("src/conversation/application/")

  if (inConversationRules && !path.endsWith(".test.ts")) {
    if (/from\s+["']react["']/.test(text) || /from\s+["']react\//.test(text)) {
      fail(file, "conversation model/use cases must not import React")
    }
    if (/@tauri-apps/.test(text)) {
      fail(file, "conversation model/use cases must not import the host")
    }
    if (/redux/i.test(text)) {
      fail(file, "conversation model/use cases must not import the store")
    }
  }

  if (path.startsWith("src/conversation/ui/")) {
    if (/application\/internal/.test(text)) {
      fail(file, "the UI reads the projection; it does not import gateway internals")
    }
    if (/from\s+["'][^"']*host-window["']/.test(text)) {
      fail(file, "the UI does not talk to the host; adapters do")
    }
    if (/Linux[A-Z]/.test(text) || /data-host=/.test(text)) {
      fail(file, "host policy belongs in src/host, as a HostFeatures field")
    }
  }

  if (
    /from\s+["']@tauri-apps(?:\/[^"']*)?["']/.test(text) ||
    /import\(\s*["']@tauri-apps/.test(text)
  ) {
    if (path !== "src/host-window.ts") {
      fail(file, "only host-window.ts may import @tauri-apps")
    }
  }

  if (!path.startsWith("src/host/") && /hostKind\s*===/.test(text)) {
    fail(file, "hostKind === belongs in src/host, as a HostFeatures field")
  }

  if (
    (path === "src/app.tsx" || path.endsWith("/app.tsx")) &&
    /\buseEffect\b/.test(text)
  ) {
    fail(file, "app.tsx renders; effects belong in a hook")
  }

  if (path === "src/store.ts" && /from\s+["']\.\/app["']/.test(text)) {
    fail(file, "the store must not import the panel chrome")
  }

  if (
    path !== draftReplyHome &&
    !path.endsWith(".test.ts") &&
    /\bdraftReply\b/.test(text)
  ) {
    fail(file, "draftReply is the stand-in runtime; only the local gateway may call it")
  }

  if ((path === "src/app.tsx" || path.endsWith("/app.tsx")) && /Linux[A-Z]/.test(text)) {
    fail(
      file,
      "host policy belongs in src/host; do not name Linux components in the chrome",
    )
  }
}

if (failures.length > 0) {
  console.error("architecture check failed:\n")
  for (const line of failures) console.error(`  ${line}`)
  process.exit(1)
}

console.log("architecture check passed")
