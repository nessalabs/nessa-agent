import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"
import { bundleArchitecture, includesDiskImage } from "./bundle-architecture.mjs"
import { parseBuildArguments, runDesktopBuild } from "./build-command.mjs"

const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
const packageConfig = JSON.parse(readFileSync("package.json", "utf8"))

test("packaged desktop content policy permits required local capabilities", () => {
  const policy = config.app.security.csp
  assert.equal(typeof policy, "string")
  const directives = Object.fromEntries(
    policy.split(";").map((directive) => {
      const [name, ...values] = directive.trim().split(/\s+/)
      return [name, values]
    }),
  )
  assert.deepEqual(directives, {
    "default-src": ["'self'"],
    "script-src": ["'self'"],
    "style-src": ["'self'", "'unsafe-inline'"],
    "font-src": ["'self'", "data:"],
    "media-src": ["'self'", "data:"],
    "img-src": ["'self'", "asset:", "http://asset.localhost", "blob:", "data:", "https:"],
    "connect-src": [
      "'self'",
      "ipc:",
      "http://ipc.localhost",
      "ws://127.0.0.1:7420",
      "http://127.0.0.1:7420",
      "ws://127.0.0.1:7421",
      "http://127.0.0.1:7421",
      "https:",
    ],
    "object-src": ["'none'"],
    "base-uri": ["'self'"],
    "frame-src": ["'none'"],
    "form-action": ["'none'"],
  })
  assert.equal(config.app.security.devCsp, null)
})

test("the updater checks published releases against a public key only", () => {
  const updater = config.plugins.updater
  assert.deepEqual(updater.endpoints, [
    "https://github.com/nessalabs/nessa-agent/releases/latest/download/latest.json",
  ])
  // Without this the bundler produces no update artifacts at all, and the
  // endpoint above would describe releases nothing can install.
  assert.equal(config.bundle.createUpdaterArtifacts, true)

  // The signing key's other half lives outside the repository. A secret key
  // pasted in here would verify signatures just as happily, so the guard is on
  // what the configured key actually says it is.
  const key = Buffer.from(updater.pubkey, "base64").toString("utf8")
  assert.match(key, /^untrusted comment: minisign public key: [0-9A-F]+\n/)
  assert.doesNotMatch(key, /secret key/i)
})

test("native bundle verification selects Intel, Apple Silicon, and universal images", () => {
  assert.equal(bundleArchitecture(undefined, "x64"), "x64")
  assert.equal(bundleArchitecture(undefined, "arm64"), "aarch64")
  assert.equal(bundleArchitecture("x86_64-apple-darwin", "arm64"), "x64")
  assert.equal(bundleArchitecture("aarch64-apple-darwin", "x64"), "aarch64")
  assert.equal(bundleArchitecture("universal-apple-darwin", "arm64"), "universal")
  assert.throws(() => bundleArchitecture(undefined, "ia32"), /Unsupported/)
  assert.throws(() => bundleArchitecture("powerpc-apple-darwin", "x64"), /Unsupported/)
})

test("disk image verification follows explicit bundles or the authoritative default", () => {
  assert.equal(config.bundle.targets, "all")
  assert.equal(includesDiskImage(undefined, config.bundle.targets), true)
  assert.equal(includesDiskImage("app", config.bundle.targets), false)
  assert.equal(includesDiskImage("app,dmg", config.bundle.targets), true)
  assert.equal(includesDiskImage("all", ["app"]), true)
})

test("the public desktop build command verifies the final bundle", () => {
  assert.equal(packageConfig.scripts["app:build"], "node scripts/desktop/build.mjs")
  const buildScript = readFileSync("scripts/desktop/build.mjs", "utf8")
  assert.match(buildScript, /runDesktopBuild/)
  assert.match(buildScript, /if \(status !== 0\) process\.exit/)
})

test("generated runtime resources exist only in the public build configuration", () => {
  assert.match(readFileSync(".gitignore", "utf8"), /^src-tauri\/runtime\/$/m)
  assert.equal(config.bundle.resources, undefined)
  const calls = []
  runDesktopBuild({
    args: [],
    environment: {},
    platform: "darwin",
    spawn(command, args, options) {
      calls.push({ command, args, options })
      return { status: 0 }
    },
  })
  const configIndex = calls[0].args.indexOf("--config")
  assert.ok(configIndex >= 0)
  assert.deepEqual(JSON.parse(calls[0].args[configIndex + 1]).bundle.resources, {
    "runtime/": "runtime/",
  })
})

test("build argument forms select the exact artifact and disk image to verify", () => {
  assert.deepEqual(
    parseBuildArguments(["--target=aarch64-apple-darwin", "--bundles=dmg,app"]),
    { target: "aarch64-apple-darwin", bundles: "dmg,app" },
  )
  assert.deepEqual(parseBuildArguments(["-t", "x86_64-apple-darwin", "-b", "app,dmg"]), {
    target: "x86_64-apple-darwin",
    bundles: "app,dmg",
  })
  assert.deepEqual(parseBuildArguments(["-tuniversal-apple-darwin", "-b=dmg"]), {
    target: "universal-apple-darwin",
    bundles: "dmg",
  })
  for (const args of [
    ["--target"],
    ["--target="],
    ["-t="],
    ["--target", "one", "--target=two"],
    ["--bundles"],
    ["--bundles="],
    ["-b", "app", "--bundles=dmg"],
  ]) {
    assert.throws(() => parseBuildArguments(args))
  }
})

test("equal-form build arguments reach final verification unchanged", () => {
  const calls = []
  const status = runDesktopBuild({
    args: ["--target=aarch64-apple-darwin", "--bundles=app,dmg"],
    environment: {
      APPLE_SIGNING_IDENTITY: "-",
      NESSA_BUILD_TARGET: "stale-target",
      NESSA_BUILD_BUNDLES: "stale-bundle",
      RETAINED: "yes",
    },
    platform: "darwin",
    spawn(command, args, options) {
      calls.push({ command, args, options })
      return { status: 0 }
    },
  })
  assert.equal(status, 0)
  // Build, then staple the disk image, then verify. The staple is between the
  // two because the bundler notarizes the app and builds the image around it,
  // leaving the image itself without a ticket for the verification to find.
  assert.equal(calls.length, 3)
  assert.deepEqual(calls[1].args, ["scripts/desktop/notarize-disk-image.mjs"])
  assert.deepEqual(calls[2].args, ["scripts/desktop/verify-bundle.mjs"])
  for (const call of calls.slice(1)) {
    assert.equal(call.options.env.NESSA_BUILD_TARGET, "aarch64-apple-darwin")
    assert.equal(call.options.env.NESSA_BUILD_BUNDLES, "app,dmg")
    assert.ok(call.options.env.NESSA_BUILD_BUNDLES.split(",").includes("dmg"))
    assert.equal(call.options.env.RETAINED, "yes")
  }
})

/** A disk image that cannot be stapled is not one to go on and verify. */
test("a failed staple stops the build before verification", () => {
  const calls = []
  const status = runDesktopBuild({
    args: ["--bundles", "app,dmg"],
    environment: { APPLE_SIGNING_IDENTITY: "-" },
    platform: "darwin",
    spawn(command, args) {
      calls.push(args.at(-1))
      return { status: args.at(-1).includes("notarize-disk-image") ? 1 : 0 }
    },
  })
  assert.equal(status, 1)
  assert.ok(
    !calls.some((call) => call.includes("verify-bundle")),
    "the bundle was verified although its disk image had no ticket",
  )
})

test("verification does not inherit artifact selectors without matching arguments", () => {
  const calls = []
  runDesktopBuild({
    args: [],
    environment: {
      NESSA_BUILD_TARGET: "stale-target",
      NESSA_BUILD_BUNDLES: "stale-bundle",
    },
    platform: "darwin",
    spawn(command, args, options) {
      calls.push({ command, args, options })
      return { status: 0 }
    },
  })
  assert.equal(calls[1].options.env.NESSA_BUILD_TARGET, undefined)
  assert.equal(calls[1].options.env.NESSA_BUILD_BUNDLES, undefined)
})

test("runtime preparation is capability-scoped to macOS packaging", async () => {
  const { prepareDesktopRuntime } = await import("./prepare.mjs")
  for (const platform of ["linux", "win32"]) {
    let loaded = false
    assert.deepEqual(
      await prepareDesktopRuntime({
        platform,
        loadManagedRuntime: async () => {
          loaded = true
        },
      }),
      { managedGateway: false },
    )
    assert.equal(loaded, false)
  }
  let loaded = false
  assert.deepEqual(
    await prepareDesktopRuntime({
      platform: "darwin",
      loadManagedRuntime: async () => {
        loaded = true
      },
    }),
    { managedGateway: true },
  )
  assert.equal(loaded, true)
})

/**
 * The CSP is a string in a JSON file and the ports are a table in another; only
 * a test can hold the two together. A packaged build names the port its own
 * stage listens on, so a stage whose port the policy does not allow produces a
 * bundle where every gateway request is blocked — visible as "gateway
 * unavailable" and a console violation, and only in a packaged build, because
 * `devCsp` is null and neither `tauri dev` nor `pnpm web` enforces this.
 */
test("the content policy reaches every port a stage can listen on", () => {
  const table = JSON.parse(readFileSync("protocol/defaults/gateway-ports.json", "utf8"))
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
  const connect = config.app.security.csp
    .split(";")
    .map((directive) => directive.trim())
    .find((directive) => directive.startsWith("connect-src"))

  for (const [stage, port] of Object.entries(table.stages)) {
    for (const scheme of ["ws", "http"]) {
      assert.ok(
        connect.includes(`${scheme}://127.0.0.1:${port}`),
        `connect-src does not allow ${scheme}://127.0.0.1:${port}, the ${stage} stage's port`,
      )
    }
  }
})
