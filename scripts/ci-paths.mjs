#!/usr/bin/env node
/**
 * Whether a pull request must run `scripts/mcp-test-server`'s tests (#476).
 *
 * Those tests spawn processes. They are not folded into `pnpm scripts:test`
 * in CI: that script also runs `nessa-ui-paths.test.mjs`, which is a separate
 * fix (#513), and the whole script is more than this check. The predicate is
 * the narrow set the server tests actually depend on:
 *
 * - `scripts/mcp-test-server/` — the server, its tests, and their fixtures
 * - `crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs` — the call id the
 *   recorded frames name
 * - `crates/nessa-sdk/tests/infrastructure/<agent>_acp/tools/fixtures/` — the
 *   recorded frames those tests replay
 * - this script and `.github/workflows/local-auth.yml` — the wiring that
 *   decides to run them, so a change to the check runs the check
 *
 * An empty list runs the tests. Nothing changed is indistinguishable here
 * from nothing being reported, and skipping the tests on a blank report
 * would hide a truncated file list.
 *
 *   git diff --name-only --no-renames base...head | node scripts/ci-paths.mjs mcp-test-server
 *
 * prints `true` or `false` on stdout.
 */
import { pathToFileURL } from "node:url"

const FIXTURE = /^crates\/nessa-sdk\/tests\/infrastructure\/[^/]+_acp\/tools\/fixtures\//

/** Whether `paths` include the MCP test server or the inputs its tests replay. */
export function mcpTestServerChanged(paths) {
  const changed = paths.map((path) => path.trim()).filter((path) => path.length > 0)
  if (changed.length === 0) return true
  return changed.some(
    (path) =>
      path.startsWith("scripts/mcp-test-server/") ||
      path === "crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs" ||
      path === "scripts/ci-paths.mjs" ||
      path === ".github/workflows/local-auth.yml" ||
      FIXTURE.test(path),
  )
}

/** Read the changed paths from stdin, one per line. */
async function readPaths(stream) {
  let text = ""
  stream.setEncoding("utf8")
  for await (const chunk of stream) text += chunk
  return text.split("\n")
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const which = process.argv[2]
  if (which !== "mcp-test-server") {
    process.stderr.write("usage: node scripts/ci-paths.mjs mcp-test-server < changed.txt\n")
    process.exit(2)
  }
  const paths = await readPaths(process.stdin)
  process.stdout.write(mcpTestServerChanged(paths) ? "true" : "false")
}
