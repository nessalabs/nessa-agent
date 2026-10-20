/**
 * The standalone `nessa` a release publishes for each target (#734), and the
 * pins the release's own builds are compiled with.
 *
 * A gateway installs itself on an SSH host that has no copy of its build. On
 * a host of its own platform it sends its own executable; on another it needs
 * that platform's build of the same sources, and has to know it is that
 * build before sending it. So a release builds each target's `nessa` first,
 * publishes those files, and compiles the app's own gateway with each one's
 * SHA-256 and length (`NESSA_HOST_BUILDS`, read by
 * `crates/nessa-server/src/env/config.rs`):
 *
 *   host-builds (one per target) ──▶ stage: nessa-<target>
 *                                        │
 *   host-pins ◀──────────────────────────┘  pins: <target>=<sha256>:<bytes>,…
 *        │
 *        ▼
 *   build (one per target, compiled with the pins) ──▶ release: every
 *                                                      nessa-<target> attached
 *
 * Every target a release builds must have its file, and nothing else may be
 * there: a pin for a file that is not published is a build no host can be
 * sent, and a file without a pin is one no gateway will send.
 *
 *   node scripts/desktop/host-builds.mjs stage --target <triple> --binary <path> --into <dir>
 *   node scripts/desktop/host-builds.mjs pins --directory <dir>
 */

import { createHash } from "node:crypto"
import { copyFileSync, mkdirSync, readdirSync, readFileSync } from "node:fs"
import { resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { option } from "./cli.mjs"
import { RELEASE_TARGETS } from "./release-assets.mjs"

/** The name a target's `nessa` is published under. */
export function hostBuildName(target) {
  if (!RELEASE_TARGETS.includes(target))
    throw new Error(`No release is built for ${target}`)
  return `nessa-${target}`
}

/**
 * The pins for the files in a directory: `<target>=<sha256>:<bytes>` for
 * every release target, in release order, comma-separated.
 *
 * @param {Record<string, Buffer>} files each file's bytes, by its name
 */
export function hostBuildPins(files) {
  const expected = RELEASE_TARGETS.map(hostBuildName)
  const missing = expected.filter((name) => !Object.hasOwn(files, name))
  if (missing.length > 0)
    throw new Error(
      `No ${missing.join(", ")}: every release target needs its host build.`,
    )
  const extra = Object.keys(files).filter((name) => !expected.includes(name))
  if (extra.length > 0)
    throw new Error(`${extra.join(", ")} is not a release target's host build.`)
  return RELEASE_TARGETS.map((target) => {
    const bytes = files[hostBuildName(target)]
    if (bytes.length === 0) throw new Error(`${hostBuildName(target)} is empty.`)
    const digest = createHash("sha256").update(bytes).digest("hex")
    return `${target}=${digest}:${bytes.length}`
  }).join(",")
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main()

function main() {
  const args = process.argv.slice(2)
  const [command] = args
  try {
    if (command === "stage") {
      const target = option(args, "target", undefined)
      const binary = option(args, "binary", undefined)
      if (!target || !binary) throw new Error("--target and --binary are required")
      const into = resolve(option(args, "into", "host-builds"))
      mkdirSync(into, { recursive: true })
      copyFileSync(resolve(binary), resolve(into, hostBuildName(target)))
      console.error(`  ${binary} -> ${hostBuildName(target)}`)
    } else if (command === "pins") {
      const directory = resolve(option(args, "directory", "host-builds"))
      const files = Object.fromEntries(
        readdirSync(directory).map((name) => [
          name,
          readFileSync(resolve(directory, name)),
        ]),
      )
      // stdout carries the pins alone: the workflow captures it.
      process.stdout.write(`${hostBuildPins(files)}\n`)
    } else {
      console.error("Usage: host-builds.mjs stage|pins [options]")
      process.exit(2)
    }
  } catch (failure) {
    console.error(failure.message)
    process.exit(1)
  }
}
