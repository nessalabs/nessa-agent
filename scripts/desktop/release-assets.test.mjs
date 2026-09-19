import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import {
  RELEASE_TARGETS,
  bundleDirectory,
  diskImageAssetName,
  releaseAssetUrl,
  releasePlatforms,
  releaseTarget,
  releaseUpdaterManifest,
  signatureBlock,
  stagedAssets,
  updaterAssetName,
} from "./release-assets.mjs"

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))

/** A well-formed signature block, in the encoding Tauri writes `.sig` files. */
/**
 * A sidecar shaped like the one Tauri writes: a comment, a signature line of
 * the right length and algorithm, then the trusted comment and its signature.
 *
 * The bytes are made up — nothing here verifies — but the shape is not, because
 * a check that only looked at the comment let a file with no signature in it
 * through, and a fixture with no signature in it could not have noticed.
 */
function signature(comment) {
  const bytes = Buffer.concat([
    Buffer.from("ED", "latin1"),
    Buffer.alloc(72, comment.charCodeAt(0) || 1),
  ])
  return Buffer.from(
    `untrusted comment: signature from tauri secret key\n` +
      `${bytes.toString("base64")}\n` +
      `trusted comment: ${comment}\n` +
      `${Buffer.alloc(64, 7).toString("base64")}\n`,
  ).toString("base64")
}

test("a release builds each macOS architecture on its own, and no universal", () => {
  // `prepare-macos.mjs` refuses a target triple that is not the host's, so
  // these two are separate runners and `universal-apple-darwin` is not an
  // option. A manifest key for a bundle nobody can build is worse than an error.
  assert.deepEqual(RELEASE_TARGETS, ["aarch64-apple-darwin", "x86_64-apple-darwin"])
  assert.equal(releaseTarget("aarch64-apple-darwin"), "darwin-aarch64")
  assert.equal(releaseTarget("x86_64-apple-darwin"), "darwin-x86_64")
  assert.throws(() => releaseTarget("universal-apple-darwin"), /No single-architecture/)
  assert.throws(() => releaseTarget("powerpc-apple-darwin"), /Unsupported/)
})

test("both architectures publish under distinct names", () => {
  // The bundler writes `Nessa.app.tar.gz` on both runners. A GitHub release has
  // one flat namespace: published as built, the second upload would replace the
  // first and one architecture would download the other's bytes, failing as a
  // rejected signature on a user's machine rather than here.
  const names = RELEASE_TARGETS.map((target) =>
    updaterAssetName("Nessa", "0.1.0", target),
  )
  assert.deepEqual(names, [
    "Nessa_0.1.0_darwin-aarch64.app.tar.gz",
    "Nessa_0.1.0_darwin-x86_64.app.tar.gz",
  ])
  assert.equal(new Set(names).size, names.length)
  assert.deepEqual(
    RELEASE_TARGETS.map((target) => diskImageAssetName("Nessa", "0.1.0", target)),
    ["Nessa_0.1.0_aarch64.dmg", "Nessa_0.1.0_x64.dmg"],
  )
})

test("staging reads the paths a targeted build actually writes", () => {
  // `--target` puts the bundle under the triple, which is what `verify-bundle`
  // reads `NESSA_BUILD_TARGET` for, and the disk image name is the bundler's
  // own — `bundle-architecture.mjs` is the single source for that spelling.
  assert.equal(
    bundleDirectory("aarch64-apple-darwin"),
    "target/aarch64-apple-darwin/release/bundle",
  )
  assert.deepEqual(stagedAssets("Nessa", "0.1.0", "x86_64-apple-darwin"), [
    {
      built: "target/x86_64-apple-darwin/release/bundle/macos/Nessa.app.tar.gz",
      published: "Nessa_0.1.0_darwin-x86_64.app.tar.gz",
    },
    {
      built: "target/x86_64-apple-darwin/release/bundle/macos/Nessa.app.tar.gz.sig",
      published: "Nessa_0.1.0_darwin-x86_64.app.tar.gz.sig",
    },
    {
      built: "target/x86_64-apple-darwin/release/bundle/dmg/Nessa_0.1.0_x64.dmg",
      published: "Nessa_0.1.0_x64.dmg",
    },
  ])
})

test("artifact URLs point at the release the tag creates", () => {
  assert.equal(
    releaseAssetUrl(
      "nessalabs/nessa-agent",
      "v0.1.0",
      "Nessa_0.1.0_darwin-aarch64.app.tar.gz",
    ),
    "https://github.com/nessalabs/nessa-agent/releases/download/v0.1.0/Nessa_0.1.0_darwin-aarch64.app.tar.gz",
  )
})

test("one manifest describes both architectures", () => {
  const manifest = releaseUpdaterManifest({
    productName: "Nessa",
    version: "0.1.0",
    tag: "v0.1.0",
    repository: "nessalabs/nessa-agent",
    targets: RELEASE_TARGETS,
    signatures: {
      "darwin-aarch64": signature("apple silicon"),
      "darwin-x86_64": signature("intel"),
    },
    notes: "Nessa 0.1.0",
    published: "2026-09-18T12:00:00.000Z",
  })
  assert.deepEqual(Object.keys(manifest.platforms), ["darwin-aarch64", "darwin-x86_64"])
  assert.equal(manifest.version, "0.1.0")
  assert.equal(new Date(manifest.pub_date).toISOString(), manifest.pub_date)
  assert.equal(
    manifest.platforms["darwin-x86_64"].url,
    "https://github.com/nessalabs/nessa-agent/releases/download/v0.1.0/Nessa_0.1.0_darwin-x86_64.app.tar.gz",
  )
  // Each key must carry its own architecture's signature. Crossed entries
  // verify nowhere and are indistinguishable from a corrupt download.
  assert.notEqual(
    manifest.platforms["darwin-aarch64"].signature,
    manifest.platforms["darwin-x86_64"].signature,
  )
})

test("a missing architecture stops the manifest rather than shrinking it", () => {
  // The failure this exists for: one runner's build fails, the release is
  // assembled from what arrived, and everyone on the missing architecture is
  // told they are up to date forever.
  assert.throws(
    () =>
      releasePlatforms({
        productName: "Nessa",
        version: "0.1.0",
        tag: "v0.1.0",
        repository: "nessalabs/nessa-agent",
        targets: RELEASE_TARGETS,
        signatures: { "darwin-aarch64": signature("apple silicon") },
      }),
    /No signature for darwin-x86_64/,
  )
})

test("an empty or garbled signature file is refused before it is published", () => {
  assert.equal(signatureBlock(`${signature("ok")}\n`, "a.sig"), signature("ok"))
  assert.throws(() => signatureBlock("   \n", "a.sig"), /is empty/)
  assert.throws(
    () => signatureBlock("not base64 at all!!", "a.sig"),
    /not base64|minisign signature/,
  )
  assert.throws(
    () => signatureBlock(Buffer.from("hello").toString("base64"), "a.sig"),
    /minisign signature/,
  )
})

/**
 * What this guard is for is catching a sidecar that cannot sign anything before
 * it is published, rather than on a user's machine at install. Checking only
 * for the header did not do that: a file holding the comment and nothing else
 * passed, and so did one with rubbish appended, which was then carried into the
 * manifest as the signature.
 */
test("a sidecar with no signature in it is refused, however it is dressed up", () => {
  const header = "untrusted comment: signature from tauri secret key\n"

  assert.throws(
    () => signatureBlock(Buffer.from(header).toString("base64"), "a.sig"),
    /no signature after it/,
  )
  // A comment, and something after it that is not a signature.
  assert.throws(
    () =>
      signatureBlock(
        Buffer.from(`${header}not a signature\n`).toString("base64"),
        "a.sig",
      ),
    /not base64|truncated/,
  )
  // A signature cut short: the right alphabet, not enough of it.
  const short = Buffer.concat([Buffer.from("ED", "latin1"), Buffer.alloc(30, 9)])
  assert.throws(
    () =>
      signatureBlock(
        Buffer.from(`${header}${short.toString("base64")}\n`).toString("base64"),
        "a.sig",
      ),
    /truncated/,
  )
  // The right length, signed by an algorithm the updater does not read.
  const wrong = Buffer.concat([Buffer.from("XX", "latin1"), Buffer.alloc(72, 9)])
  assert.throws(
    () =>
      signatureBlock(
        Buffer.from(`${header}${wrong.toString("base64")}\n`).toString("base64"),
        "a.sig",
      ),
    /ed25519/,
  )
  // And rubbish after a well-formed file is not quietly kept.
  assert.throws(
    () => signatureBlock(`${signature("ok")}!!!not base64!!!`, "a.sig"),
    /not base64/,
  )
})

/** The real thing, from `tauri signer sign`, is accepted unchanged. */
test("a signature this release process actually produces is accepted", () => {
  const real =
    "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUMTJRSTRJZS9UL1J3WDJCWWh6NXFtL2RzS1FqQVZ1Z2dGeWpQTXc2ZEk5dXdHSllVdDlPRXRnY0N1aUNRQWdhR0xlSWpiZ3ltcEJhS0FjM1NjRG5FZFp6RFFISEJTdWdBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg5Njk4NjU4CWZpbGU6cGF5bG9hZC5iaW4KUWVxMzZhaG1uODlyNkJrUHhWbGxYRitTU2twc0pCNGs2ay8yMVRKTmZOUGNmWlhRQm9ZQU4vT0ZDeWgzSWJMSlZ3ZjhyTEwrMDVyZzJoQnFHTjU1RFE9PQo="

  assert.equal(signatureBlock(`${real}\n`, "a.sig"), real)
})

test("published names follow the shipped product and version", () => {
  // Read from the config the app itself reports, so a release cannot name its
  // assets one version while the bundle inside them reports another.
  assert.equal(
    updaterAssetName(config.productName, config.version, "aarch64-apple-darwin"),
    `Nessa_${config.version}_darwin-aarch64.app.tar.gz`,
  )
  assert.equal(config.bundle.createUpdaterArtifacts, true)
})
