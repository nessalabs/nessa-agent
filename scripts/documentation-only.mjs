#!/usr/bin/env node
/**
 * Decide whether a change can skip the compile-heavy CI jobs.
 *
 * A full run of `local-auth.yml` is around thirty minutes and four runners,
 * nearly all of it `rustc`. A pull request that edits prose pays that in full
 * for checks that cannot read what it changed. This is the gate that says so.
 *
 * The rule is an allow-list, not a deny-list, and it is deliberately one line
 * long: a path is inert when it is Markdown. Everything else runs everything.
 * A gate that skips work has to be wrong in only one direction, and the way to
 * keep it that way is to keep the set of things it will skip on small enough to
 * hold in your head.
 *
 * Markdown qualifies because nothing in this repository reads it:
 *
 *   - No `include_str!` names a `.md` file, so no Rust crate embeds one and no
 *     doc test runs out of one. `cargo doc -p nessa-sdk -D warnings` compiles
 *     doc comments in `.rs` sources; `crates/nessa-sdk/docs/*.md` reaches it
 *     only if something includes it, and nothing does.
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
 * README, a check starts parsing a document — this rule is what has to change
 * with it, and `documentation-only.test.mjs` is where to say so.
 *
 *   git diff --name-only --no-renames base...head | node scripts/documentation-only.mjs
 *
 * prints `true` or `false` on stdout, for a workflow to put in `$GITHUB_OUTPUT`.
 */
import { pathToFileURL } from "node:url"

/** Paths CI cannot form an opinion about, so a change confined to them is inert. */
function inert(path) {
  return path.endsWith(".md")
}

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
  return changed.every(inert)
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
