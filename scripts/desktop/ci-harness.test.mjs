import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("local-auth integration harness is locked to its declared tools", () => {
  const manifest = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package.json", "utf8"),
  )
  const lock = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package-lock.json", "utf8"),
  )
  assert.deepEqual(lock.packages[""].dependencies, manifest.dependencies)
  assert.deepEqual(manifest.dependencies, {
    "@nessa/client": "file:./packages/nessa-client",
    tsx: "4.23.13",
    ws: "8.18.3",
  })
  assert.deepEqual(lock.packages["node_modules/@nessa/client"], {
    resolved: "packages/nessa-client",
    link: true,
  })
  for (const dependency of ["tsx", "ws"]) {
    const installed = lock.packages[`node_modules/${dependency}`]
    assert.equal(installed.version, manifest.dependencies[dependency])
    assert.match(installed.integrity, /^sha512-/)
  }
})

test("local and CI aggregate the same named frontend and native checks", () => {
  const root = JSON.parse(readFileSync("package.json", "utf8"))
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.match(root.scripts.check, /pnpm frontend:check/)
  for (const [aggregate, script, command] of [
    ["check", "frontend:check", "pnpm frontend:check"],
    ["sdk:check", "sdk:docs:check", "node scripts/check-sdk-docs.mjs"],
    ["check", "mcp:check", "node scripts/check-mcp.mjs"],
    ["check", "desktop:check", "node scripts/check-desktop.mjs"],
  ]) {
    assert.ok(root.scripts[aggregate].includes(`pnpm ${script}`))
    assert.ok(workflow.includes(`run: ${command}`), `CI is missing ${script}`)
  }
  assert.match(
    workflow,
    /cargo fmt -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk -- --check/,
  )
  assert.equal(
    root.scripts.architecture,
    "node --test scripts/architecture/*.test.mjs && node scripts/check-architecture.mjs",
  )
  assert.match(workflow, /node --test scripts\/architecture\/\*\.test\.mjs/)
  assert.match(workflow, /node scripts\/check-architecture\.mjs/)
  assert.match(
    workflow,
    /cargo test -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk/,
  )
  assert.match(
    workflow,
    /cargo clippy -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk --all-targets -- -D warnings/,
  )
  assert.match(workflow, /npm ci --ignore-scripts/)
})
