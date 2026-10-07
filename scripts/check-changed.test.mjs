import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import test from "node:test"

import { changedPaths, orderCommands, parseCheckScript, plan } from "./check-changed.mjs"
import { documentationOnly } from "./documentation-only.mjs"

test("an empty list runs the full local check", () => {
  assert.deepEqual(plan([]), {
    tier: "full",
    commands: ["check"],
    reason: "an empty file list is not a narrowed check",
  })
  assert.equal(plan(["", "  "]).tier, "full")
})

test("Markdown alone runs nothing locally and agrees with the CI classifier", () => {
  const paths = ["README.md", "docs/ARCHITECTURE.md"]
  const result = plan(paths)
  assert.equal(documentationOnly(paths), true)
  assert.equal(result.tier, "docs")
  assert.deepEqual(result.commands, [])
  const mixed = [...paths, "src/panel/ui/app.tsx"]
  assert.equal(documentationOnly(mixed), false)
  assert.equal(plan(mixed).tier, "narrow")
})

test("a panel change runs the frontend contract and not Rust", () => {
  assert.deepEqual(plan(["src/panel/ui/app.tsx", "README.md"]).commands, [
    "frontend:check",
  ])
})

test("frontend paths and one crate union in pnpm check order", () => {
  assert.deepEqual(
    plan([
      "crates/nessa-sdk/src/lib.rs",
      "src/panel/ui/app.tsx",
      "crates/nessa-auth/src/lib.rs",
    ]).commands,
    ["frontend:check", "auth:fmt:check", "auth:clippy", "auth:test", "sdk:check"],
  )
})

test("paths this table does not own run the full local check", () => {
  for (const path of [
    "src-tauri/src/main.rs",
    "Cargo.lock",
    "Cargo.toml",
    "package.json",
    "pnpm-lock.yaml",
    "protocol/defaults/gateway-ports.json",
    "scripts/check-changed.mjs",
    ".github/workflows/local-auth.yml",
    "crates/nessa-gateway-endpoint/src/lib.rs",
    "crates/nessa-app/src/main.rs",
  ]) {
    const result = plan([path])
    assert.equal(result.tier, "full", path)
    assert.deepEqual(result.commands, ["check"])
  }
})

test("each owned crate maps to the package script CI already runs for it", () => {
  assert.deepEqual(plan(["crates/nessa-server/src/lib.rs"]).commands, [
    "server:fmt:check",
    "server:clippy",
    "server:test",
  ])
  assert.deepEqual(plan(["crates/nessa-protocol/src/lib.rs"]).commands, [
    "server:fmt:check",
    "server:clippy",
    "server:test",
  ])
  assert.deepEqual(plan(["crates/nessa-mcp/src/lib.rs"]).commands, ["mcp:check"])
  assert.deepEqual(plan(["crates/nessa-images/src/lib.rs"]).commands, ["images:check"])
  assert.deepEqual(plan(["crates/nessa-local-database/src/lib.rs"]).commands, [
    "database:check",
  ])
  assert.deepEqual(plan(["scripts/agents/pin-agents.mjs"]).commands, ["agents:check"])
  assert.deepEqual(plan(["docs/generated/client-api.json"]).commands, ["frontend:check"])
})

test("a dotted path is owned only after it is normalized", () => {
  assert.equal(plan(["src/../Cargo.lock"]).tier, "full")
  assert.equal(plan(["scripts/agents/../../Cargo.lock"]).tier, "full")
  assert.equal(plan(["src/../src-tauri/src/main.rs"]).tier, "full")
  assert.deepEqual(plan(["./src/panel/ui/app.tsx"]).commands, ["frontend:check"])
  assert.deepEqual(plan(["src\\panel\\ui\\app.tsx"]).commands, ["frontend:check"])
})

test("a selected script missing from the check order runs the full local check", () => {
  const order = ["frontend:check", "images:check"]
  assert.deepEqual(orderCommands(new Set(["frontend:check"]), order), ["frontend:check"])
  assert.equal(orderCommands(new Set(["desktop:check"]), order), null)
  assert.equal(orderCommands(new Set(["images:check", "desktop:check"]), order), null)
})

test("pnpm check owns the narrow command order", () => {
  const script = JSON.parse(readFileSync("package.json", "utf8")).scripts.check
  const commands = parseCheckScript(script)
  assert.equal(commands.map((command) => `pnpm ${command}`).join(" && "), script)
  assert.equal(parseCheckScript("cargo test"), null)
  assert.equal(plan(["src/panel/ui/app.tsx\nCargo.lock"]).tier, "full")
})

test("a src prefix does not claim src-tauri", () => {
  assert.equal(plan(["src-tauri/tauri.conf.json"]).tier, "full")
  assert.equal(plan(["src/host/window.ts"]).tier, "narrow")
})

test("git discovery includes the branch diff and untracked files", () => {
  const calls = []
  const git = (args) => {
    calls.push(args.join(" "))
    if (args[0] === "merge-base" && args[2] === "origin/main") return "abc"
    if (args[0] === "diff") return "src/panel/ui/app.tsx\n"
    if (args[0] === "ls-files") return "src/new.tsx\n"
    return null
  }
  assert.deepEqual(changedPaths(git), ["src/panel/ui/app.tsx", "src/new.tsx"])
  assert.equal(calls[0], "merge-base HEAD origin/main")
})

test("git discovery fails closed when the base cannot be named", () => {
  assert.deepEqual(
    changedPaths(() => null),
    [],
  )
  assert.equal(plan(changedPaths(() => null)).tier, "full")
})

test("an empty pipe is an empty list, and arguments do not need --", () => {
  const empty = spawnSync(process.execPath, ["scripts/check-changed.mjs", "--plan"], {
    input: "",
    encoding: "utf8",
  })
  assert.equal(empty.status, 0)
  assert.equal(empty.stderr, "")
  assert.equal(
    JSON.parse(empty.stdout).reason,
    "an empty file list is not a narrowed check",
  )

  const named = spawnSync(
    process.execPath,
    ["scripts/check-changed.mjs", "--plan", "src/panel/ui/app.tsx"],
    { encoding: "utf8" },
  )
  assert.equal(named.status, 0)
  assert.deepEqual(JSON.parse(named.stdout).commands, ["frontend:check"])
})

test("the plan command prints JSON on stdout and does not run pnpm", () => {
  const result = spawnSync(
    process.execPath,
    ["scripts/check-changed.mjs", "--plan", "--", "src/panel/ui/app.tsx"],
    { encoding: "utf8" },
  )
  assert.equal(result.status, 0)
  assert.equal(result.stderr, "")
  assert.deepEqual(JSON.parse(result.stdout), plan(["src/panel/ui/app.tsx"]))
})

test("required CI does not call the local narrower", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.equal(workflow.includes("check-changed"), false)
  assert.equal(workflow.includes("check:changed"), false)
})
