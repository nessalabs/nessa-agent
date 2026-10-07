#!/usr/bin/env node
/**
 * Run the local checks that own the paths in a change, or the full local check.
 *
 * Required CI is not this script. `local-auth.yml` still runs every job unless
 * `scripts/documentation-only.mjs` says the change is Markdown. A narrow plan
 * here is only for a person or an agent waiting on a checkout. An empty list,
 * an unknown path, or a path this table does not own runs `pnpm check`.
 *
 * Markdown is inert only by asking `inertPath` in `documentation-only.mjs`.
 * That function is the owner of the extension rule.
 *
 *   git diff --name-only --no-renames base...head | node scripts/check-changed.mjs --plan
 *   node scripts/check-changed.mjs --plan src/panel/ui/app.tsx
 *   pnpm check:changed
 *
 * `--plan` prints one JSON object on stdout and runs nothing. Without it, the
 * plan is a line on stderr and the selected scripts inherit stdout. An empty
 * pipe is an empty list, which runs `pnpm check`. Paths are the arguments,
 * with or without `--`. A terminal with no arguments reads the git diff.
 */
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import path from "node:path"
import { pathToFileURL } from "node:url"

import { inertPath } from "./documentation-only.mjs"

/**
 * The `pnpm check` chain, in its order. `package.json` owns that sequence.
 * A union of groups keeps it. A crate-only change does not run the scripts
 * ahead of it.
 */
export function parseCheckScript(script) {
  if (typeof script !== "string" || script.length === 0) return null
  const commands = []
  for (const part of script.split("&&")) {
    const match = part.trim().match(/^pnpm ([A-Za-z0-9:_-]+)$/)
    if (!match) return null
    commands.push(match[1])
  }
  return commands.length > 0 ? commands : null
}

const ORDER = parseCheckScript(
  JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).scripts
    .check,
)
if (!ORDER) throw new Error("package.json scripts.check is not a chain of pnpm scripts")

const GROUPS = [
  {
    prefixes: ["src/", "packages/", "verification/"],
    exact: [
      "eslint.config.js",
      "prettier.config.js",
      ".prettierignore",
      "vitest.config.ts",
      "tsconfig.json",
      "typedoc.client.json",
      "docs/generated/client-api.json",
    ],
    commands: ["frontend:check"],
  },
  {
    prefixes: ["scripts/agents/"],
    commands: ["agents:check"],
  },
  {
    prefixes: [
      "crates/nessa-server/",
      "crates/nessa-protocol/",
      "crates/nessa-client-core/",
    ],
    commands: ["server:fmt:check", "server:clippy", "server:test"],
  },
  {
    prefixes: ["crates/nessa-local-storage/", "crates/nessa-auth/"],
    commands: ["auth:fmt:check", "auth:clippy", "auth:test"],
  },
  { prefixes: ["crates/nessa-sdk/"], commands: ["sdk:check"] },
  { prefixes: ["crates/nessa-mcp/"], commands: ["mcp:check"] },
  { prefixes: ["crates/nessa-local-database/"], commands: ["database:check"] },
  { prefixes: ["crates/nessa-images/"], commands: ["images:check"] },
]

function full(reason) {
  return { tier: "full", commands: ["check"], reason }
}

function commandsFor(changed) {
  for (const group of GROUPS) {
    const prefix = group.prefixes?.some((item) => changed.startsWith(item))
    const exact = group.exact?.includes(changed)
    if (prefix || exact) return group.commands
  }
  return null
}

/**
 * Git paths are slash-separated. A `--` argument is not, so collapse `.` and
 * `..` before the prefix table sees them. A path that still leaves the
 * repository is unowned.
 */
export function normalizeChangedPath(input) {
  if (input.includes("\n") || input.includes("\r") || input.includes("\0")) return null
  const normalized = path.posix.normalize(input.replaceAll("\\", "/"))
  if (
    normalized.length === 0 ||
    normalized === "." ||
    normalized === ".." ||
    normalized.startsWith("../") ||
    normalized.startsWith("/")
  )
    return null
  return normalized
}

/** Selected scripts in `order`, or null when one of them is not in that list. */
export function orderCommands(selected, order = ORDER) {
  const commands = order.filter((command) => selected.has(command))
  if (commands.length !== selected.size) return null
  return commands
}

/**
 * @param {string[]} paths
 * @returns {{ tier: "docs" | "narrow" | "full", commands: string[], reason: string }}
 */
export function plan(paths) {
  const changed = []
  for (const raw of paths) {
    const trimmed = raw.trim()
    if (trimmed.length === 0) continue
    const normalized = normalizeChangedPath(trimmed)
    if (normalized === null) return full(`no narrow check owns ${trimmed}`)
    changed.push(normalized)
  }
  if (changed.length === 0) return full("an empty file list is not a narrowed check")
  const substantive = changed.filter((path) => !inertPath(path))
  if (substantive.length === 0) {
    return {
      tier: "docs",
      commands: [],
      reason: "every path is Markdown; local compile checks cannot see them",
    }
  }
  const selected = new Set()
  for (const path of substantive) {
    const commands = commandsFor(path)
    if (commands === null) return full(`no narrow check owns ${path}`)
    for (const command of commands) selected.add(command)
  }
  const commands = orderCommands(selected)
  if (commands === null)
    return full("the narrow plan dropped a script; running the full local check")
  return {
    tier: "narrow",
    commands,
    reason:
      "narrowed to the package scripts that own these paths; required CI still runs every job",
  }
}

/** Working tree against merge-base with origin/main, plus untracked files. */
export function changedPaths(git) {
  const bases = ["origin/main", "main"]
  let base = null
  for (const candidate of bases) {
    const found = git(["merge-base", "HEAD", candidate])
    if (found) {
      base = found
      break
    }
  }
  if (!base) return []
  const diff = git(["diff", "--name-only", "--no-renames", base])
  const untracked = git(["ls-files", "--others", "--exclude-standard"])
  if (diff === null || untracked === null) return []
  return [...diff.split("\n"), ...untracked.split("\n")]
    .map((path) => path.trim())
    .filter((path) => path.length > 0)
}

function gitOutput(args) {
  const result = spawnSync("git", args, { encoding: "utf8" })
  if (result.status !== 0) return null
  return result.stdout.trim()
}

async function readStdin(stream) {
  let text = ""
  stream.setEncoding("utf8")
  for await (const chunk of stream) text += chunk
  return text.split("\n")
}

function runScripts(commands) {
  for (const command of commands) {
    const result = spawnSync("pnpm", [command], { stdio: "inherit" })
    if (result.error) throw result.error
    if ((result.status ?? 1) !== 0) return result.status ?? 1
  }
  return 0
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const argv = process.argv.slice(2)
  const planOnly = argv[0] === "--plan"
  const rest = planOnly ? argv.slice(1) : argv
  let paths
  if (rest[0] === "--") {
    paths = rest.slice(1)
  } else if (rest.length > 0) {
    paths = rest
  } else if (!process.stdin.isTTY) {
    paths = await readStdin(process.stdin)
  } else {
    paths = changedPaths(gitOutput)
  }
  const result = plan(paths)
  if (planOnly) {
    process.stdout.write(`${JSON.stringify(result)}\n`)
  } else {
    process.stderr.write(`${result.reason}\n`)
    if (result.commands.length > 0) process.exitCode = runScripts(result.commands)
  }
}
