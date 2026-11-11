#!/usr/bin/env node
/**
 * Decide which Rust work a pull request can affect, for `local-auth.yml`.
 *
 * `documentation-only.mjs` answers one question: can this change skip every
 * compile job? This script answers the next ones, for a change that is code:
 *
 *   - which of the crates the Rust test and clippy steps list can it
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
 *      members' own `Cargo.toml` files. A member's `Cargo.toml` itself is not
 *      such a file: Cargo merges features across every crate a command lists,
 *      so one manifest's dependency change can change how unrelated crates
 *      build. It reaches every crate and every platform.
 *   2. A file under the frontend roots (`src/`, `packages/`, `verification/`,
 *      `docs/`, `experiments/`, or a web or config file at the repository root,
 *      and a Markdown file a Rust test reads, see `documentation-only.mjs`)
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
 * Platform tests. The macOS and Windows legs keep running clippy on every Rust
 * change, which is what catches a `#[cfg(unix)]` item left dead on Windows or
 * an import that only one system has. Their test suites run when a reached
 * Rust file is platform code: its text has a `cfg` on a platform, `target_os`,
 * `std::os` or a system crate (`PLATFORM_CODE`); a module above it is declared
 * under such a `cfg`; its path names a platform folder or file; or its diff
 * adds or removes a line that mentions one. They also run when the diff is not
 * available (the files API leaves `patch` out of large and binary files) and on
 * every global change.
 *
 * This is the one place the plan can be wrong in the silent direction, and it
 * is a choice: shared code that names no platform but behaves differently on
 * one (a path separator, a line ending) is tested on Linux and linted on all
 * three systems, and its macOS and Windows tests wait for the push to `main`,
 * which runs everything. A break that gets through is caught there and fixed
 * forward. Running the two platform suites for every Rust change would keep
 * the macOS leg at its full fifty minutes for most pull requests.
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
import { appendFileSync, readFileSync, readdirSync, statSync } from "node:fs"
import { join, relative, sep } from "node:path"
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
  "nessa-projection",
  "nessa-types",
  "nessa-client-core",
  "nessa-device-client",
  "nessa-sdk",
  "nessa-timer",
  "nessa-vault",
  "nessa-agent-install",
  "nessa-attachments",
  "nessa-peer-gateways",
  "nessa-mcp-authorization",
  "nessa-webhooks",
  "nessa-device-pairing",
  "nessa-env-serve",
]

/** The files API returns at most this many files; a list that long may be cut. */
export const FILES_API_LIMIT = 3000

const FRONTEND_ROOTS = ["src/", "packages/", "verification/", "docs/", "experiments/"]
const ROOT_FRONTEND_FILE =
  /^[^/]+\.(html|ts|tsx|js|mjs|cjs|json|ya?ml|css)$|^nessa-ui-revision$|^\.prettierignore$/
const PLATFORM_PATH =
  /(^|[/_.-])(windows|win32|macos|darwin|linux|unix|platform)([/_.-]|$)/i
/** Rust that compiles or behaves differently per system. */
const PLATFORM_WORDS =
  "unix|windows|target_os|target_family|target_env|target_arch|target_vendor"
const SYSTEM_CRATES = "libc|nix|windows_sys|windows|objc2\\w*|core_foundation\\w*|cocoa"
// A `cfg` may be wrapped over several lines by rustfmt, so everything up to
// the attribute's closing `]` counts.
export const PLATFORM_CODE = new RegExp(
  `\\bcfg(?:_attr)?!?\\s*\\([^\\]]{0,400}?\\b(?:${PLATFORM_WORDS})\\b|\\bstd::os::|\\b(?:${SYSTEM_CRATES})::|CARGO_CFG_(?:TARGET|UNIX|WINDOWS)`,
)
/**
 * A changed line that mentions a platform: at least what `PLATFORM_CODE` sees.
 * No word boundary in front, so `CARGO_CFG_TARGET_OS` in a build script counts.
 */
const PLATFORM_LINE = new RegExp(
  `cfg!?\\s*\\(|(?:${PLATFORM_WORDS}|macos)\\b|std::os::|(?:${SYSTEM_CRATES})::`,
  "i",
)
const PLATFORM_MODULE = new RegExp(
  `#\\[cfg(?:_attr)?\\s*\\([^\\]]*?\\b(?:${PLATFORM_WORDS})\\b[^\\]]*\\]\\s*((?:(?:#\\[[^\\]]*\\]|//[^\\n]*\\n)\\s*)*)(?:pub(?:\\([^)]*\\))?\\s+)?mod\\s+(\\w+)`,
  "g",
)

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
 * @param {{ dir: string, manifest: string, text: string, platformText?: string, platformFiles?: Iterable<string> }[]} members
 *   each member's directory relative to the repository root, its `Cargo.toml`,
 *   the string literals of its Rust sources and build script joined, and the
 *   repository paths of its Rust files that are platform code. A path ending in
 *   `/` stands for every file under it: the folder of a module declared under
 *   a platform `cfg`. `platformText` is the literals of those files alone, so
 *   a data file only platform code reads runs the platform suites too.
 */
export function workspaceGraph(members) {
  const byDir = new Map()
  for (const member of members) {
    const { name, paths } = memberManifest(member.manifest)
    byDir.set(member.dir, {
      name,
      dir: member.dir,
      paths,
      text: member.text,
      platformText: member.platformText ?? "",
      platformFiles: [...(member.platformFiles ?? [])],
    })
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

/** Whether a Rust file is platform code by its path or its crate's scan. */
function platformPath(member, path) {
  return (
    PLATFORM_PATH.test(path.slice(member.dir.length)) ||
    member.platformFiles.some((platform) =>
      platform.endsWith("/") ? path.startsWith(platform) : path === platform,
    )
  )
}

/** Why a reached Rust file needs the macOS and Windows suites, or null. */
function platformReason(member, path, file) {
  const inside = path.slice(member.dir.length)
  if (PLATFORM_PATH.test(inside)) return "names a platform"
  if (!path.endsWith(".rs"))
    return member.platformText.includes(path.split("/").at(-1))
      ? "is named by platform code"
      : null
  if (typeof file.patch !== "string") return "no diff to read"
  if (platformPath(member, path)) return "is platform code"
  if (changedLines(file.patch).some((line) => PLATFORM_LINE.test(line)))
    return "changes platform-specific code"
  return null
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
      reasons: ["documentation only"],
    }

  const reached = new Set()
  const reasons = []
  let platformTests = false
  for (const file of files) {
    for (const path of [file.filename, file.previous_filename]) {
      if (typeof path !== "string" || path.trim() === "" || inertPath(path)) continue
      const member = memberOf(graph, path)
      if (member && path !== `${member.dir}/Cargo.toml`) {
        reached.add(member.name)
        reasons.push(`${path}: in ${member.name}`)
        const platform = platformReason(member, path, file)
        if (platform) {
          platformTests = true
          reasons.push(`${path}: ${platform}`)
        }
        continue
      }
      if (!member && (frontendPath(path) || path.endsWith(".md"))) {
        const names = namesFor(path)
        for (const candidate of graph.members) {
          if (names.some((name) => candidate.text.includes(name))) {
            if (names.some((name) => candidate.platformText.includes(name))) {
              platformTests = true
              reasons.push(`${path}: named by platform code in ${candidate.name}`)
            }
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

/**
 * The files of the modules one Rust source declares under a platform `cfg`,
 * as repository paths: each module's file and its folder, which holds its own
 * submodules. Everything in such a module compiles on one system only.
 *
 * @param {string} file the declaring file's repository path
 */
export function platformModuleFiles(file, source) {
  const folder = file.slice(0, file.lastIndexOf("/") + 1)
  const stem = file.slice(folder.length).replace(/\.rs$/, "")
  // A crate root (`lib.rs`, `main.rs`, but also `tests/x.rs`, `build.rs` or a
  // `[[bin]]` path) holds its modules beside it; any other file in a folder
  // named after it. Which one a file is needs its manifest, so both count.
  const homes = ["mod", "lib", "main"].includes(stem)
    ? [folder]
    : [folder, `${folder}${stem}/`]
  const files = []
  for (const match of source.matchAll(PLATFORM_MODULE)) {
    const path = match[1].match(/#\[path\s*=\s*"([^"]+)"/)
    const targets = path
      ? [normalise(`${folder}${path[1]}`)]
      : homes.map((home) => `${home}${match[2]}.rs`)
    for (const target of targets)
      files.push(target, `${target.replace(/(\/mod)?\.rs$/, "")}/`)
  }
  return files
}

/** What `workspaceGraph` needs from one member's Rust sources on disk. */
function memberSources(root, dir) {
  const literals = []
  const platformFiles = []
  const literalsByFile = new Map()
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name === "target" || entry.name === "node_modules") continue
      const path = join(current, entry.name)
      if (entry.isDirectory()) walk(path)
      else if (entry.name.endsWith(".rs")) {
        const source = readFileSync(path, "utf8")
        const file = relative(root, path).split(sep).join("/")
        const named = stringLiterals(source)
        literals.push(...named)
        literalsByFile.set(file, named)
        if (PLATFORM_CODE.test(source)) platformFiles.push(file)
        platformFiles.push(...platformModuleFiles(file, source))
      }
    }
  }
  walk(join(root, dir))
  // Second pass: what platform code names, once every platform file is known,
  // including the files of platform modules and platform-named paths.
  const platformLiterals = []
  for (const [file, named] of literalsByFile)
    if (platformPath({ dir, platformFiles }, file)) platformLiterals.push(...named)
  return {
    text: literals.join("\n"),
    platformText: platformLiterals.join("\n"),
    platformFiles,
  }
}

/** Read the workspace from a checkout. */
export function readWorkspace(root) {
  const members = workspaceMembers(readFileSync(join(root, "Cargo.toml"), "utf8")).map(
    (dir) => ({
      dir,
      manifest: readFileSync(join(root, dir, "Cargo.toml"), "utf8"),
      ...(statSync(join(root, dir)).isDirectory()
        ? memberSources(root, dir)
        : { text: "" }),
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
  const event = process.env.EVENT ?? "pull_request"
  const root =
    relative(process.cwd(), fileURLToPath(new URL("..", import.meta.url))) || "."
  // Anything but a pull request runs everything, so it neither needs nor
  // depends on reading the workspace.
  const plan = planChange(
    parseInput(input, args.includes("--files-jsonl")),
    event === "pull_request" ? readWorkspace(root) : null,
    { event },
  )
  process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`)
  if (args.includes("--github-output")) {
    const output = process.env.GITHUB_OUTPUT
    if (!output) throw new Error("GITHUB_OUTPUT is not set")
    appendFileSync(output, githubOutput(plan))
  }
}
