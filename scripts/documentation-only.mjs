#!/usr/bin/env node
/**
 * Decide whether a change can skip the compile-heavy CI jobs.
 *
 * A full run of `local-auth.yml` is around thirty minutes and four runners,
 * nearly all of it `rustc`. A pull request that edits prose pays that in full
 * for checks that cannot read what it changed. This is the gate that says so.
 *
 * The rule is an allow-list, not a deny-list, and it is deliberately short: a
 * path is inert when it is Markdown that nothing reads. Everything else runs
 * everything. A gate that skips work has to be wrong in only one direction, and
 * the way to keep it that way is to keep the set of things it will skip on
 * small enough to hold in your head.
 *
 * Markdown qualifies because, with the exceptions in `READ_BY_RUST`, nothing in
 * this repository reads it:
 *
 *   - No `include_str!` names a `.md` file, so no Rust crate embeds one and no
 *     doc test runs out of one. `cargo doc -p nessa-sdk -D warnings` compiles
 *     doc comments in `.rs` sources; `crates/nessa-sdk/docs/*.md` reaches it
 *     only if something includes it, and nothing does.
 *   - A Rust test may open a Markdown file at run time and compare it, as
 *     `nessa-server`'s limits test does with `docs/limits.md`. Each such file
 *     is listed in `READ_BY_RUST`, and a change to it is code.
 *   - `scripts/check-architecture.mjs` enforces the contract *described in*
 *     `docs/ARCHITECTURE.md` and `docs/codebase-structure.md`, but it reads
 *     source to do it. It never opens either file.
 *   - Prettier formats `src scripts packages` and two config files. No Markdown.
 *
 * `docs/` as a whole does NOT qualify, which is why this is about the
 * extension and not the directory: `docs/generated/client-api.json` is
 * regenerated and compared by `scripts/check-client-docs.mjs`, so a change
 * there is a change CI has an opinion about.
 *
 * When one of those facts stops being true — a crate starts including its
 * README, a test starts reading another document — this rule is what has to
 * change with it. `documentation-only.test.mjs` scans the Rust sources for both
 * and fails until `READ_BY_RUST` says so.
 *
 *   git diff --name-only --no-renames base...head | node scripts/documentation-only.mjs
 *
 * prints `true` or `false` on stdout, for a workflow to put in `$GITHUB_OUTPUT`.
 */
import { pathToFileURL } from "node:url"

/**
 * Paths CI cannot form an opinion about, so a change confined to them is inert.
 *
 * `scripts/check-changed.mjs` asks this same function. A second copy of the
 * extension check would be a second owner of the decision.
 */
export function inertPath(path) {
  return path.endsWith(".md") && !READ_BY_RUST.has(path)
}

/**
 * Markdown files a Rust test reads at run time, by repository path. The test
 * beside this file holds the list equal to what the Rust sources name.
 */
export const READ_BY_RUST = new Set(["docs/limits.md"])

/**
 * Whether every changed path is inert.
 *
 * An empty list is not documentation-only. Nothing changed is indistinguishable
 * here from nothing being *reported* as changed — a diff against the wrong base,
 * an API call that returned nothing — and the two want opposite answers. The
 * one that runs the checks is the one that cannot be silently wrong.
 */
export function documentationOnly(paths) {
  const changed = paths.map((path) => path.trim()).filter((path) => path.length > 0)
  if (changed.length === 0) return false
  return changed.every(inertPath)
}

/** Read the changed paths from stdin, one per line. */
async function readPaths(stream) {
  let text = ""
  stream.setEncoding("utf8")
  for await (const chunk of stream) text += chunk
  return text.split("\n")
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const paths = await readPaths(process.stdin)
  process.stdout.write(documentationOnly(paths) ? "true" : "false")
}
