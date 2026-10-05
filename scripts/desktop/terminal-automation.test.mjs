import assert from "node:assert/strict"
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { execFileSync } from "node:child_process"
import { tmpdir } from "node:os"
import { resolve } from "node:path"
import test from "node:test"
import { verifyTerminalAutomation } from "./terminal-automation.mjs"

const permission = "com.apple.security.automation.apple-events"
const purpose = "NSAppleEventsUsageDescription"

test("the shipped bundle inputs declare the permission requested by Terminal login", () => {
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
  assert.equal(config.bundle.macOS.hardenedRuntime, true)
  const entitlement = readFileSync(
    resolve("src-tauri", config.bundle.macOS.entitlements),
    "utf8",
  )
  assert.match(
    entitlement,
    /<key>com\.apple\.security\.automation\.apple-events<\/key>\s*<true\/>/,
  )
  const info = readFileSync(resolve("src-tauri", config.bundle.macOS.infoPlist), "utf8")
  assert.match(
    info,
    /<key>NSAppleEventsUsageDescription<\/key>\s*<string>[^<]+<\/string>/,
  )
})

function verify(entitlements, info) {
  const calls = []
  verifyTerminalAutomation("/built/Nessa.app", (command, args, options) => {
    calls.push([command, args, options])
    if (command === "/usr/bin/codesign") return "effective signed entitlements"
    if (options.input) {
      assert.equal(options.input, "effective signed entitlements")
      return JSON.stringify(entitlements)
    }
    return JSON.stringify(info)
  })
  return calls
}

test("release verification reads the app signature and effective bundled purpose", () => {
  const calls = verify({ [permission]: true }, { [purpose]: "Open Terminal for login" })
  assert.deepEqual(calls[0][1], [
    "--display",
    "--entitlements",
    "-",
    "--xml",
    "/built/Nessa.app",
  ])
  assert.equal(calls[2][1].at(-1), resolve("/built/Nessa.app", "Contents/Info.plist"))
})

test("missing signed permission or effective purpose refuses bundle verification", () => {
  for (const entitlement of [{}, { [permission]: false }])
    assert.throws(
      () => verify(entitlement, { [purpose]: "Open Terminal" }),
      /does not permit/,
    )
  for (const info of [{}, { [purpose]: " " }, { [purpose]: true }])
    assert.throws(() => verify({ [permission]: true }, info), /does not explain/)
})

test(
  "effective signed bundle declarations survive packaging and reject omissions",
  {
    skip: process.platform !== "darwin",
  },
  () => {
    const temporary = mkdtempSync(resolve(tmpdir(), "nessa-terminal-automation-"))
    try {
      const app = resolve(temporary, "Nessa.app")
      const contents = resolve(app, "Contents")
      const executable = resolve(contents, "MacOS/Nessa")
      mkdirSync(resolve(contents, "MacOS"), { recursive: true })
      copyFileSync("/usr/bin/true", executable)
      const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
      const info = resolve(contents, "Info.plist")
      const entitlement = resolve(temporary, "Entitlements.plist")
      copyFileSync(resolve("src-tauri", config.bundle.macOS.infoPlist), info)
      copyFileSync(resolve("src-tauri", config.bundle.macOS.entitlements), entitlement)
      for (const [key, value] of [
        ["CFBundleIdentifier", "so.nessa.automation-test"],
        ["CFBundleExecutable", "Nessa"],
        ["CFBundlePackageType", "APPL"],
      ])
        execFileSync("/usr/bin/plutil", ["-insert", key, "-string", value, info])
      const sign = () =>
        execFileSync(
          "/usr/bin/codesign",
          [
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
            entitlement,
            app,
          ],
          { stdio: "pipe" },
        )
      sign()
      verifyTerminalAutomation(app, execFileSync)
      execFileSync("/usr/bin/plutil", [
        "-remove",
        permission.replaceAll(".", "\\."),
        entitlement,
      ])
      sign()
      assert.throws(() => verifyTerminalAutomation(app, execFileSync), /does not permit/)
      copyFileSync(resolve("src-tauri", config.bundle.macOS.entitlements), entitlement)
      execFileSync("/usr/bin/plutil", ["-remove", purpose, info])
      sign()
      assert.throws(() => verifyTerminalAutomation(app, execFileSync), /does not explain/)
    } finally {
      rmSync(temporary, { recursive: true, force: true })
    }
  },
)
