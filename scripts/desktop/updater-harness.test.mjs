import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import {
  CHECK_ONLY_ARTIFACT,
  CHECK_ONLY_SIGNATURE,
  checkOnlyManifest,
  defaultArtifacts,
  option,
  releaseManifest,
  updaterTarget,
} from "./updater-manifest.mjs"

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))

test("harness target keys are spelled the way the updater plugin spells them", () => {
  // `updater_os` in tauri-plugin-updater answers "darwin" for macOS and the
  // Rust architecture names, none of which are what Node reports. A key that is
  // merely plausible produces no update and no error.
  assert.equal(updaterTarget("darwin", "arm64"), "darwin-aarch64")
  assert.equal(updaterTarget("darwin", "x64"), "darwin-x86_64")
  assert.equal(updaterTarget("linux", "x64"), "linux-x86_64")
  assert.equal(updaterTarget("win32", "x64"), "windows-x86_64")
  assert.equal(updaterTarget("linux", "arm"), "linux-armv7")
  assert.throws(() => updaterTarget("aix", "x64"), /platform/)
  assert.throws(() => updaterTarget("darwin", "mips"), /architecture/)
})

test("the served manifest carries every field the plugin's release reader requires", () => {
  const manifest = releaseManifest({
    version: "0.1.0",
    notes: "local",
    platforms: {
      "darwin-aarch64": {
        signature: "c2lnbmF0dXJl",
        url: "http://localhost:7430/Nessa.app.tar.gz",
      },
    },
    published: "2026-09-17T12:00:00.000Z",
  })
  assert.deepEqual(manifest, {
    version: "0.1.0",
    notes: "local",
    pub_date: "2026-09-17T12:00:00.000Z",
    platforms: {
      "darwin-aarch64": {
        signature: "c2lnbmF0dXJl",
        url: "http://localhost:7430/Nessa.app.tar.gz",
      },
    },
  })
  // `pub_date` is parsed as RFC 3339 before the platform is looked at, so a
  // non-conforming date rejects the whole manifest rather than one platform.
  assert.equal(new Date(manifest.pub_date).toISOString(), manifest.pub_date)
})

test("one manifest carries every architecture a release publishes", () => {
  // The two macOS architectures are built on different runners and converge
  // here: `releases/latest/download/latest.json` is a single file, and an
  // architecture missing from it is an architecture that can never update.
  const manifest = releaseManifest({
    version: "0.1.0",
    notes: "release",
    platforms: {
      "darwin-aarch64": { signature: "YQ==", url: "https://example.invalid/a" },
      "darwin-x86_64": { signature: "Yg==", url: "https://example.invalid/b" },
    },
    published: "2026-09-17T12:00:00.000Z",
  })
  assert.deepEqual(Object.keys(manifest.platforms), ["darwin-aarch64", "darwin-x86_64"])
  assert.equal(manifest.platforms["darwin-x86_64"].url, "https://example.invalid/b")
})

test("a manifest that would apply to nothing, or verify nothing, is refused", () => {
  const release = { version: "0.1.0", notes: "", published: "2026-09-17T12:00:00.000Z" }
  // An empty map parses fine and reports "no update available" on every
  // machine, which is the failure this whole module exists to make loud.
  assert.throws(
    () => releaseManifest({ ...release, platforms: {} }),
    /applies to nothing/,
  )
  assert.throws(
    () =>
      releaseManifest({
        ...release,
        platforms: {
          "darwin-aarch64": { signature: "", url: "https://example.invalid/a" },
        },
      }),
    /No signature for darwin-aarch64/,
  )
  assert.throws(
    () =>
      releaseManifest({
        ...release,
        platforms: { "darwin-aarch64": { signature: "YQ==", url: "" } },
      }),
    /No artifact URL for darwin-aarch64/,
  )
})

test("a check-only manifest is complete enough to check and honest about the rest", () => {
  const manifest = checkOnlyManifest({
    version: "99.0.0",
    notes: "local",
    target: "darwin-aarch64",
    origin: "http://127.0.0.1:7430",
    published: "2026-09-17T12:00:00.000Z",
  })
  // Everything the plugin's *check* reads is genuinely there, in the shape it
  // reads it: a missing field or a bad date makes the check fail rather than
  // succeed, which would test nothing.
  assert.equal(manifest.version, "99.0.0")
  assert.equal(new Date(manifest.pub_date).toISOString(), manifest.pub_date)
  assert.deepEqual(Object.keys(manifest.platforms), ["darwin-aarch64"])
  assert.equal(
    manifest.platforms["darwin-aarch64"].url,
    `http://127.0.0.1:7430/${CHECK_ONLY_ARTIFACT}`,
  )
  // And the part that is not tested says what it is, in words, in the manifest
  // itself — so a captured check-only manifest cannot be mistaken for a signed
  // one by anyone who opens it.
  assert.match(
    Buffer.from(manifest.platforms["darwin-aarch64"].signature, "base64").toString(),
    /not a signature/,
  )
  assert.equal(manifest.platforms["darwin-aarch64"].signature, CHECK_ONLY_SIGNATURE)
})

test("check-only announces a version above the shipped one and says where it stops", () => {
  const harness = readFileSync("scripts/desktop/updater-harness.mjs", "utf8")
  // The dev app reports the shipped version, so the announced one has to beat
  // it for the check to produce an offer at all.
  assert.ok(
    Number(harness.match(/checkOnly \? "(\d+)\.0\.0"/)[1]) >
      Number(config.version.split(".")[0]),
  )
  assert.match(harness, /pnpm tauri dev --config/)
  assert.match(harness, /What this mode does NOT test/)
})

test("the harness looks for artifacts the release build is configured to produce", () => {
  assert.equal(config.bundle.createUpdaterArtifacts, true)
  assert.deepEqual(defaultArtifacts("darwin", config.version), [
    "target/release/bundle/macos/Nessa.app.tar.gz",
  ])
  assert.deepEqual(defaultArtifacts("linux", "0.1.0"), [
    "target/release/bundle/appimage/Nessa_0.1.0_amd64.AppImage.tar.gz",
  ])
  assert.deepEqual(defaultArtifacts("win32", "0.1.0"), [
    "target/release/bundle/nsis/Nessa_0.1.0_x64-setup.exe",
  ])
  assert.throws(() => defaultArtifacts("aix", "0.1.0"), /platform/)
})

test("harness options read both argument forms and reject a missing value", () => {
  assert.equal(option(["--port", "7431"], "port", "7430"), "7431")
  assert.equal(option(["--port=7432"], "port", "7430"), "7432")
  assert.equal(option([], "port", "7430"), "7430")
  assert.equal(option(["--artifact"], "port", undefined), undefined)
  assert.throws(() => option(["--port", "--version"], "port", "7430"), /requires a value/)
  assert.throws(() => option(["--port"], "port", "7430"), /requires a value/)
})

test("local redirection stays a build-time merge and never enters the shipped config", () => {
  // The whole point of the harness is that pointing the updater somewhere else
  // costs a `--config` flag on one build, not a setting in the product.
  assert.deepEqual(config.plugins.updater.endpoints, [
    "https://github.com/nessalabs/nessa-agent/releases/latest/download/latest.json",
  ])
  const harness = readFileSync("scripts/desktop/updater-harness.mjs", "utf8")
  assert.match(harness, /pnpm app:build --config/)
  assert.doesNotMatch(harness, /writeFileSync\([^)]*tauri\.conf\.json/)
})
