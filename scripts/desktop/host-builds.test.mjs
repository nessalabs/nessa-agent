import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { hostBuildName, hostBuildPins } from "./host-builds.mjs"
import { RELEASE_TARGETS } from "./release-assets.mjs"

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex")

function builds() {
  return Object.fromEntries(
    RELEASE_TARGETS.map((target) => [
      hostBuildName(target),
      Buffer.from(`nessa for ${target}`),
    ]),
  )
}

test("each release target's host build is pinned by its digest and length, in release order", () => {
  const files = builds()
  assert.equal(
    hostBuildPins(files),
    RELEASE_TARGETS.map((target) => {
      const bytes = files[`nessa-${target}`]
      return `${target}=${sha256(bytes)}:${bytes.length}`
    }).join(","),
  )
})

test("a target without its host build, an unknown file or an empty one stops the release", () => {
  const missing = builds()
  delete missing["nessa-x86_64-apple-darwin"]
  assert.throws(() => hostBuildPins(missing), /No nessa-x86_64-apple-darwin/)
  assert.throws(
    () =>
      hostBuildPins({ ...builds(), "nessa-aarch64-unknown-linux-gnu": Buffer.from("x") }),
    /not a release target's host build/,
  )
  assert.throws(
    () =>
      hostBuildPins({ ...builds(), "nessa-x86_64-unknown-linux-gnu": Buffer.alloc(0) }),
    /is empty/,
  )
  assert.throws(() => hostBuildName("universal-apple-darwin"), /No release is built/)
})

test("the release builds every target's host build first and compiles the app with their pins", () => {
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  const rows = [...workflow.matchAll(/^\s+host-target: (\S+)$/gm)].map(
    ([, target]) => target,
  )
  assert.deepEqual(rows, RELEASE_TARGETS)
  // The gateway itself, from the tagged commit, and nothing set that would
  // make it differ from what the app's builds compile.
  assert.match(
    workflow,
    /cargo build --locked --release -p nessa-server --bin nessa --target \$\{\{ matrix\.host-target \}\}/,
  )
  assert.match(workflow, /needs: \[version, updater-key, host-pins\]/)
  assert.match(workflow, /NESSA_HOST_BUILDS: \$\{\{ needs\.host-pins\.outputs\.pins \}\}/)
  assert.match(
    workflow,
    /NESSA_HOST_BUILDS_FROM: https:\/\/github\.com\/\$\{\{ github\.repository \}\}\/releases\/download\/\$\{\{ needs\.version\.outputs\.tag \}\}/,
  )
  // Published beside the bundles, under the names the gateway fetches.
  assert.match(workflow, /pattern: host-build-\*\s+path: release-assets/)
})
