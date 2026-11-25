import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const bootstrap = fileURLToPath(
  new URL("../../docs/guides/headless-gateway/nessa-gateway-bootstrap", import.meta.url),
)

test("an unhealthy gateway is idle only for a confirmed stopped unit", (t) => {
  const bin = mkdtempSync(join(tmpdir(), "nessa-unit-"))
  t.after(() => rmSync(bin, { recursive: true, force: true }))
  writeFileSync(join(bin, "curl"), "#!/bin/sh\nexit 1\n", { mode: 0o755 })
  writeFileSync(
    join(bin, "systemctl"),
    `#!/bin/sh
[ "$*" = "show --property=LoadState,ActiveState nessa-server.service" ] || exit 1
printf '%s\\n' "$UNIT_STATE"
exit "$QUERY_STATUS"
`,
    { mode: 0o755 },
  )
  for (const [load, active, status, busy] of [
    ["loaded", "inactive", 0, false],
    ["loaded", "inactive", 1, true],
    ["not-found", "inactive", 0, true],
    ["loaded", "activating", 0, true],
    ["loaded", "active", 0, true],
    ["loaded", "deactivating", 0, true],
    ["loaded", "failed", 0, true],
  ]) {
    const result = execFileSync("bash", [bootstrap, "busy"], {
      env: {
        ...process.env,
        PATH: bin + ":" + process.env.PATH,
        UNIT_STATE: `LoadState=${load}\nActiveState=${active}`,
        QUERY_STATUS: String(status),
      },
    })
    assert.deepEqual(JSON.parse(result), { busy }, `${load}/${active}/${status}`)
  }
})

test("healthy gateways need complete, well-formed activity evidence", (t) => {
  const bin = mkdtempSync(join(tmpdir(), "nessa-activity-"))
  t.after(() => rmSync(bin, { recursive: true, force: true }))
  writeFileSync(join(bin, "curl"), "#!/bin/sh\nexit 0\n", { mode: 0o755 })
  const nessa = join(bin, "nessa")
  writeFileSync(
    nessa,
    `#!/bin/sh
case "$2" in
  terminal.list) echo "$TERMINALS" ;;
  conversation.list)
    case "$4" in
      *true*) echo "$ARCHIVED" ;;
      *) echo "$CONVERSATIONS" ;;
    esac ;;
  *) exit 1 ;;
esac
`,
    { mode: 0o755 },
  )
  for (const [rows, terminals, complete, busy, archived = []] of [
    [[{ running: false }], [{ state: "exited" }], true, false],
    [[{ running: true }], [], true, true],
    [[], [], true, true, [{ running: true }]],
    [[], [{ state: "running" }], true, true],
    [[], [], false, true],
    [[{}], [], true, true],
    [[{ running: "false" }], [], true, true],
    [{}, [], true, true],
    [[], {}, true, true],
  ]) {
    const result = execFileSync("bash", [bootstrap, "busy"], {
      env: {
        ...process.env,
        PATH: bin + ":" + process.env.PATH,
        NESSA_BIN: nessa,
        CONVERSATIONS: JSON.stringify({ complete, conversations: rows }),
        ARCHIVED: JSON.stringify({ complete: true, conversations: archived }),
        TERMINALS: JSON.stringify({ terminals }),
      },
    })
    assert.deepEqual(
      JSON.parse(result),
      { busy },
      JSON.stringify([rows, terminals, complete]),
    )
  }
})
