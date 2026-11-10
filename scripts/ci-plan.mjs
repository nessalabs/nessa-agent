#!/usr/bin/env node
/**
 * Decide which Rust work a pull request can affect, for `local-auth.yml`.
 *
 * `documentation-only.mjs` answers one question: can this change skip every
 * compile job? This script answers the next ones, for a change that is code:
 *
 *   - which of the nine crates the Rust test and clippy steps list can it
 *     reach (`rustPackages`), and so whether those steps run at all;
 *   - whether the SDK is among them, which is all the SDK coverage job tests;
 *   - whether it needs the macOS and Windows test suites, or whether a clippy
 *     run on those systems is enough (`platformTests`).
 *
 * Like the Markdown rule, every answer here may be wrong in one direction only.
 * Running a suite that could not fail costs minutes; skipping one that would
 * have failed is silent. So anything this script does not recognise reaches
 * every crate and every platform, and the narrowing is limited to two cases it
 * can see whole:
 *
 *   1. A file inside a workspace member reaches that member and every member
 *      that depends on it (normal, dev or build dependency), read from the
 *      members' own `Cargo.toml` files.
 *   2. A file under the frontend roots (`src/`, `packages/`, `verification/`,
 *      `docs/`, `experiments/`, or a web or config file at the repository root)
 *      reaches a member only when a string literal in that member's Rust source
 *      names it: its file name, or a folder of two or more segments that holds
 *      it. Rust reads such files with `include_str!` (`src/host/window.ts` in
 *      `src-tauri`) and from build scripts (`src/desktop/styles.css`), and both
 *      spell the path in a literal. Comments are left out on purpose: the
 *      server's tests cite `verification/desktop/...` in prose all the time
 *      without reading it. Inside literals the match is a substring search, so
 *      it errs toward running.
 *
 * Everything else — `Cargo.lock`, the root `Cargo.toml`, `rust-toolchain.toml`,
 * `protocol/`, `scripts/`, `.github/`, a path in no member — reaches every
 * crate and every platform.
 *
 * Platform tests. The macOS and Windows legs keep running clippy, which is what
 * catches a `#[cfg(unix)]` item left dead on Windows or an import that only one
 * system has. Their test suites run when a reached Rust file's diff adds or
 * removes a line that mentions `cfg`, `target_os`, `std::os`, `unix`, `windows`
 * or `macos`; when a changed path names a platform folder or file; when the
 * diff is not available (the files API leaves `patch` out of large and binary
 * files); and on every global change. A push to `main` runs everything, so a
 * platform break this lets through is caught on `main` and fixed forward.
 *
 * Input on stdin is either one path per line (a person at a terminal: no diffs,
 * so platform tests always run) or, with `--files-jsonl`, one JSON object per
 * line as the pull request files API returns them: `filename`,
 * `previous_filename`, `patch`. With `--github-output` the answers are also
 * written as `key=value` lines to `$GITHUB_OUTPUT`.
 *
 *   gh api --paginate repos/o/r/pulls/1/files --jq '.[] | @json' \
 *     | node scripts/ci-plan.mjs --files-jsonl --github-output
 */
import { readFileSync, readdirSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

import { documentationOnly, inertPath } from "./documentation-only.mjs"

/**
 * The crates the Rust test and clippy steps of `local-auth` and
 * `windows-server-tests` list, in their order. The workflow keeps the same list
 * as its fallback for a missing answer; `ci-result.test.mjs` holds the two equal.
 */
export const RUST_TEST_PACKAGES = [
  "nessa-local-storage",
  "nessa-auth",
  "nessa-server",
  "nessa-protocol",
  "nessa-client-core",
  "nessa-device-client",
  "nessa-sdk",
  "nessa-timer",
  "nessa-vault",
]

/** The files API returns at most this many files; a list that long may be cut. */
export const FILES_API_LIMIT = 3000

const FRONTEND_ROOTS = ["src/", "packages/", "verification/", "docs/", "experiments/"]
const ROOT_FRONTEND_FILE =
  /^[^/]+\.(html|ts|tsx|js|mjs|cjs|json|ya?ml|css)$|^nessa-ui-revision$|^\.prettierignore$/
const PLATFORM_PATH =
  /(^|[/_.-])(windows|win32|macos|darwin|linux|unix|platform)([/_.-]|$)/i
const PLATFORM_LINE = /cfg!?\s*\(|target_os|std::os::|\bunix\b|\bwindows\b|\bmacos\b/i

/** The `[[workspace]] members` of a root `Cargo.toml`. */
export function workspaceMembers(rootManifest) {
  const match = rootManifest.match(/^\s*members\s*=\s*\[([\s\S]*?)\]/m)
  if (!match) throw new Error("the root Cargo.toml has no workspace members")
  return [...match[1].matchAll(/"([^"]+)"/g)].map((item) => item[1].replace(/\/+$/, ""))
}

/** A member manifest's package name and the paths of its path dependencies. */
export function memberManifest(manifest) {
  const pkg = manifest.match(/^\[package\][\s\S]*?^\s*name\s*=\s*"([^"]+)"/m)
  if (!pkg) throw new Error("a member Cargo.toml has no package name")
  const paths = [...manifest.matchAll(/\bpath\s*=\s*"([^"]+)"/g)].map((item) => item[1])
  return { name: pkg[1], paths }
}

function normalise(path) {
  const parts = []
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue
    if (part === "..") parts.pop()
    else parts.push(part)
  }
  return parts.join("/")
}

/**
 * The workspace as this script needs it.
 *
 * @param {{ dir: string, manifest: string, text: string }[]} members each
 *   member's directory relative to the repository root, its `Cargo.toml`, and
 *   the string literals of its Rust sources and build script, joined.
 */
export function workspaceGraph(members) {
  const byDir = new Map()
  for (const member of members) {
    const { name, paths } = memberManifest(member.manifest)
    byDir.set(member.dir, { name, dir: member.dir, paths, text: member.text })
  }
  const dependents = new Map(
    [...byDir.values()].map((member) => [member.name, new Set()]),
  )
  for (const member of byDir.values()) {
    for (const path of member.paths) {
      const target = byDir.get(normalise(`${member.dir}/${path}`))
      if (target) dependents.get(target.name).add(member.name)
    }
  }
  return { members: [...byDir.values()], dependents }
}

function memberOf(graph, path) {
  return graph.members.find((member) => path.startsWith(`${member.dir}/`))
}

function frontendPath(path) {
  return (
    FRONTEND_ROOTS.some((root) => path.startsWith(root)) || ROOT_FRONTEND_FILE.test(path)
  )
}

/** What a member's source would have to say to be reading this file. */
function namesFor(path) {
  const parts = path.split("/")
  const names = [parts.at(-1)]
  for (let end = 2; end < parts.length; end += 1)
    names.push(parts.slice(0, end).join("/"))
  return names
}

function changedLines(patch) {
  return patch
    .split("\n")
    .filter((line) => /^[+-]/.test(line) && !/^(\+\+\+|---) /.test(line))
}

/**
 * The plan for one change.
 *
 * @param {{ filename: string, previous_filename?: string, patch?: string }[]} files
 * @param {ReturnType<typeof workspaceGraph>} graph
 * @param {{ event?: string }} [options] `pull_request` narrows; anything else runs all.
 */
export function planChange(files, graph, { event = "pull_request" } = {}) {
  const everything = (reason) => ({
    code: true,
    rust: true,
    rustPackages: [...RUST_TEST_PACKAGES],
    sdk: true,
    platformTests: true,
    windowsServerTests: true,
    reasons: [reason],
  })
  const paths = files.flatMap((file) =>
    [file.filename, file.previous_filename].filter((path) => typeof path === "string"),
  )
  if (event !== "pull_request") return everything(`${event} runs everything`)
  if (files.length === 0) return everything("no changed files were reported")
  if (files.length >= FILES_API_LIMIT) return everything("the file list may be cut")
  if (documentationOnly(paths))
    return {
      code: false,
      rust: false,
      rustPackages: [],
      sdk: false,
      platformTests: false,
      windowsServerTests: false,
      reasons: ["documentation only"],
    }

  const reached = new Set()
  const reasons = []
  let platformTests = false
  for (const file of files) {
    for (const path of [file.filename, file.previous_filename]) {
      if (typeof path !== "string" || path.trim() === "" || inertPath(path)) continue
      const member = memberOf(graph, path)
      if (member) {
        reached.add(member.name)
        reasons.push(`${path}: in ${member.name}`)
        const rustish = /\.rs$|(^|\/)Cargo\.toml$/.test(path)
        if (PLATFORM_PATH.test(path.slice(member.dir.length))) {
          platformTests = true
          reasons.push(`${path}: names a platform`)
        } else if (rustish && typeof file.patch !== "string") {
          platformTests = true
          reasons.push(`${path}: no diff to read`)
        } else if (
          rustish &&
          changedLines(file.patch).some((line) => PLATFORM_LINE.test(line))
        ) {
          platformTests = true
          reasons.push(`${path}: changes platform-specific code`)
        }
        continue
      }
      if (frontendPath(path)) {
        const names = namesFor(path)
        for (const candidate of graph.members) {
          if (names.some((name) => candidate.text.includes(name))) {
            reached.add(candidate.name)
            reasons.push(`${path}: named by ${candidate.name}`)
          }
        }
        continue
      }
      const all = everything(`${path}: reaches every crate`)
      return { ...all, reasons: [...reasons, ...all.reasons] }
    }
  }

  const queue = [...reached]
  while (queue.length > 0) {
    const name = queue.pop()
    for (const dependent of graph.dependents.get(name) ?? []) {
      if (reached.has(dependent)) continue
      reached.add(dependent)
      queue.push(dependent)
    }
  }
  const rustPackages = RUST_TEST_PACKAGES.filter((name) => reached.has(name))
  const rust = rustPackages.length > 0
  return {
    code: true,
    rust,
    rustPackages,
    sdk: reached.has("nessa-sdk"),
    platformTests: rust && platformTests,
    windowsServerTests: rust && platformTests && reached.has("nessa-server"),
    reasons,
  }
}

/** `-p a -p b`, the form the workflow's cargo commands take. */
export function packageArguments(names) {
  return names.map((name) => `-p ${name}`).join(" ")
}

/** The `$GITHUB_OUTPUT` lines for a plan. */
export function githubOutput(plan) {
  return [
    `code=${plan.code}`,
    `rust=${plan.rust}`,
    `rust_packages=${packageArguments(plan.rustPackages)}`,
    `sdk=${plan.sdk}`,
    `platform_tests=${plan.platformTests}`,
    `windows_server_tests=${plan.windowsServerTests}`,
    "",
  ].join("\n")
}

/**
 * The double-quoted string literals in Rust source, raw strings included.
 * A comment is not a literal unless it quotes one, so prose that cites a
 * path does not count as reading it.
 */
export function stringLiterals(source) {
  const withoutComments = source.replace(/^\s*\/\/.*$/gm, "")
  return [...withoutComments.matchAll(/"(?:[^"\\\n]|\\.)*"/g)].map((match) => match[0])
}

function rustText(root, dir) {
  const chunks = []
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name === "target" || entry.name === "node_modules") continue
      const path = join(current, entry.name)
      if (entry.isDirectory()) walk(path)
      else if (entry.name.endsWith(".rs"))
        chunks.push(...stringLiterals(readFileSync(path, "utf8")))
    }
  }
  walk(join(root, dir))
  return chunks.join("\n")
}

/** Read the workspace from a checkout. */
export function readWorkspace(root) {
  const members = workspaceMembers(readFileSync(join(root, "Cargo.toml"), "utf8")).map(
    (dir) => ({
      dir,
      manifest: readFileSync(join(root, dir, "Cargo.toml"), "utf8"),
      text: statSync(join(root, dir)).isDirectory() ? rustText(root, dir) : "",
    }),
  )
  return workspaceGraph(members)
}

function parseInput(text, jsonl) {
  const lines = text.split("\n").filter((line) => line.trim() !== "")
  if (!jsonl) return lines.map((line) => ({ filename: line.trim() }))
  return lines.map((line) => {
    const file = JSON.parse(line)
    if (!file || typeof file.filename !== "string")
      throw new Error("each changed file needs a filename")
    return file
  })
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const args = process.argv.slice(2)
  const unknown = args.filter(
    (arg) => !["--files-jsonl", "--github-output"].includes(arg),
  )
  if (unknown.length > 0) {
    process.stderr.write(`usage: ci-plan.mjs [--files-jsonl] [--github-output]\n`)
    process.exit(2)
  }
  let input = ""
  process.stdin.setEncoding("utf8")
  for await (const chunk of process.stdin) input += chunk
  const root =
    relative(process.cwd(), fileURLToPath(new URL("..", import.meta.url))) || "."
  const plan = planChange(
    parseInput(input, args.includes("--files-jsonl")),
    readWorkspace(root),
    {
      event: process.env.EVENT ?? "pull_request",
    },
  )
  process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`)
  if (args.includes("--github-output")) {
    const output = process.env.GITHUB_OUTPUT
    if (!output) throw new Error("GITHUB_OUTPUT is not set")
    const { appendFileSync } = await import("node:fs")
    appendFileSync(output, githubOutput(plan))
  }
}
