import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { createServer } from "node:http"
import { connect } from "node:net"
import test from "node:test"
import {
  CHECK_ONLY_ARTIFACT,
  CHECK_ONLY_SIGNATURE,
  checkOnlyManifest,
  defaultArtifacts,
  option,
  releaseManifest,
  requestedPath,
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
    target: "darwin-aarch64",
    signature: "c2lnbmF0dXJl",
    url: "http://localhost:7430/Nessa.app.tar.gz",
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
  // Both shapes the bundler can write: `createUpdaterArtifacts: true` leaves
  // the AppImage alone, `"v1Compatible"` also archives it, and the plugin
  // installs either. Discovery must not depend on which one a build chose.
  assert.deepEqual(defaultArtifacts("linux", "0.1.0"), [
    "target/release/bundle/appimage/Nessa_0.1.0_amd64.AppImage",
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

/**
 * The plugin's `validate_endpoints` returns `InsecureTransportProtocol` for a
 * non-https endpoint in a release build, and only warns in a debug one. The
 * harness serves over http, so the release path it prints needs that
 * permission — and a check-only run, which is a debug build, would never have
 * shown its absence.
 */
test("the generated config permits the loopback endpoint, and the product does not", () => {
  const harness = readFileSync("scripts/desktop/updater-harness.mjs", "utf8")
  assert.match(harness, /dangerousInsecureTransportProtocol: true/)
  // Granted where it is generated, beside the endpoint it is granted for.
  assert.match(
    harness,
    /const localEndpoint = \{[\s\S]*?dangerousInsecureTransportProtocol: true[\s\S]*?\}/,
  )
  // And nowhere near what ships.
  assert.equal(config.plugins.updater.dangerousInsecureTransportProtocol, undefined)
  assert.ok(
    config.plugins.updater.endpoints.every((endpoint) => endpoint.startsWith("https://")),
  )
})

const ORIGIN = "http://127.0.0.1:7777"

test("a request target the harness cannot read is not a path it serves", () => {
  assert.equal(requestedPath("/nessa.app.tar.gz", ORIGIN), "/nessa.app.tar.gz")
  assert.equal(requestedPath("/nessa%20one.app.tar.gz", ORIGIN), "/nessa one.app.tar.gz")
  // Two different throws, neither caught in a Node request handler: `new URL`
  // on a target that is not one, `decodeURIComponent` on a bad escape. Both
  // took the harness down mid-validation before they went behind one door.
  for (const malformed of ["//[", "http://a b/", "/%ZZ", "/%", "/%E0%A4%A"])
    assert.equal(requestedPath(malformed, ORIGIN), undefined)
})

/**
 * The end state, not a proxy for it: the malformed request is answered *and*
 * the harness is still there to answer the next one. Driven through a real
 * server, because what broke was the request callback, not the parser.
 */
test("a malformed request is answered and the harness serves the next one", async () => {
  const manifest = Buffer.from(`{"version":"9.9.9"}\n`)
  const server = createServer((request, response) => {
    const asked = requestedPath(request.url, `http://127.0.0.1`)
    if (asked !== "/latest.json") {
      response.writeHead(404).end()
      return
    }
    response.writeHead(200, { "content-type": "application/json" })
    response.end(manifest)
  })
  await new Promise((ready) => server.listen(0, "127.0.0.1", ready))
  const origin = `http://127.0.0.1:${server.address().port}`

  /** Raw, so a target `fetch` would refuse to send still reaches the server. */
  const send = (target) =>
    new Promise((settled, failed) => {
      const socket = connect(server.address().port, "127.0.0.1", () => {
        socket.write(`GET ${target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n`)
      })
      let seen = ""
      socket.on("data", (chunk) => {
        seen += chunk
        if (seen.includes("\r\n")) {
          socket.destroy()
          settled(seen.split("\r\n")[0])
        }
      })
      socket.on("error", failed)
    })

  try {
    for (const malformed of ["//[", "/%ZZ"])
      assert.match(await send(malformed), /404/, `${malformed} is answered`)
    const good = await fetch(`${origin}/latest.json`)
    assert.equal(good.status, 200)
    assert.equal((await good.json()).version, "9.9.9")
  } finally {
    await new Promise((closed) => server.close(closed))
  }
})
