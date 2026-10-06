import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import test from "node:test"

import { mcpTestServerChanged } from "./ci-paths.mjs"

test("the MCP test server, its stand-in, and its recorded fixtures select the check", () => {
  assert.equal(mcpTestServerChanged(["scripts/mcp-test-server/server.test.mjs"]), true)
  assert.equal(mcpTestServerChanged(["scripts/mcp-test-server/fixtures/one.json"]), true)
  assert.equal(
    mcpTestServerChanged(["crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs"]),
    true,
  )
  assert.equal(
    mcpTestServerChanged([
      "crates/nessa-sdk/tests/infrastructure/claude_acp/tools/fixtures/mcp_live_frames.json",
    ]),
    true,
  )
  assert.equal(
    mcpTestServerChanged([
      "crates/nessa-sdk/tests/infrastructure/codex_acp/tools/fixtures/mcp_live_frames.json",
    ]),
    true,
  )
  assert.equal(mcpTestServerChanged(["scripts/ci-paths.mjs"]), true)
  assert.equal(mcpTestServerChanged([".github/workflows/local-auth.yml"]), true)
})

test("an unrelated path does not select the check, and an empty report does", () => {
  assert.equal(mcpTestServerChanged(["src/desktop/main.tsx"]), false)
  assert.equal(mcpTestServerChanged(["verification/desktop/CHECKLIST.md"]), false)
  assert.equal(
    mcpTestServerChanged([
      "crates/nessa-sdk/tests/infrastructure/claude_acp/tools/mcp_live.rs",
    ]),
    false,
  )
  assert.equal(mcpTestServerChanged([]), true)
  assert.equal(mcpTestServerChanged(["", "  "]), true)
})

test("the process prints true or false and rejects an unknown selector", () => {
  const root = new URL("..", import.meta.url)
  const run = (args, input) =>
    spawnSync(process.execPath, ["scripts/ci-paths.mjs", ...args], {
      cwd: root,
      input,
      encoding: "utf8",
    })
  const selected = run(["mcp-test-server"], "scripts/mcp-test-server/server.mjs\n")
  assert.equal(selected.status, 0)
  assert.equal(selected.stdout, "true")
  assert.equal(selected.stderr, "")

  const skipped = run(["mcp-test-server"], "README.md\n")
  assert.equal(skipped.status, 0)
  assert.equal(skipped.stdout, "false")

  const unknown = run(["other"], "")
  assert.equal(unknown.status, 2)
  assert.match(unknown.stderr, /mcp-test-server/)
})

test("gateway-contract runs the MCP test server from the path output", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/local-auth.yml", import.meta.url),
    "utf8",
  )
  assert.match(workflow, /mcp_test_server:/)
  assert.match(workflow, /node scripts\/ci-paths\.mjs mcp-test-server/)
  assert.match(workflow, /scripts\/ci-paths\.mjs/)
  assert.match(
    workflow,
    /node --test scripts\/mcp-test-server\/\*\.test\.mjs/,
  )
  assert.match(
    workflow,
    /github\.event_name != 'pull_request' \|\| needs\.changes\.outputs\.mcp_test_server == 'true'/,
  )
})
