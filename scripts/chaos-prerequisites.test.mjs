// Exercise the workflow's actual guard, including failure before and after build.
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { runInNewContext } from "node:vm"
import test from "node:test"

const workflow = readFileSync(
  new URL("../.github/workflows/local-auth.yml", import.meta.url),
  "utf8",
)
const start = workflow.indexOf("  conversation-stress:\n")
const end = workflow.indexOf("\n  windows-server-tests:", start)
assert(start !== -1 && end > start)
const job = workflow.slice(start, end)
const chaos = job.slice(job.indexOf("      - name: Run the conversation chaos check\n"))
const condition = chaos.match(/^        if: \$\{\{ (.+) \}\}$/m)?.[1]
assert(condition)

function admitted(cancelled, outcome) {
  // An explicit status function lets chaos still run after stress fails.
  assert.match(condition, /cancelled\(\)/)
  const expression = condition
    .replaceAll("cancelled()", String(cancelled))
    .replaceAll("steps.gateway-build.outcome", JSON.stringify(outcome))
  // This guard uses only boolean operators and string literals, not JS code.
  assert.match(expression, /^[\s!&|=()"'a-z_]+$/)
  return runInNewContext(expression, Object.create(null), { timeout: 100 })
}

test("the readiness outcome belongs to the existing gateway build before stress", () => {
  assert.match(
    job,
    /- id: gateway-build\n        run: cargo build -p nessa-server -p nessa-mcp\n/,
  )
  assert(
    job.indexOf("- id: gateway-build") < job.indexOf("Run the conversation stress check"),
  )
})

for (const outcome of ["skipped", "failure", "cancelled"]) {
  test(`does not invoke chaos when the gateway build was ${outcome}`, () => {
    assert.equal(admitted(false, outcome), false)
  })
}

test("still invokes chaos after stress assertion failure once the gateway was built", () => {
  assert.equal(admitted(false, "success"), true)
})

test("does not invoke chaos after cancellation even with a completed build", () => {
  assert.equal(admitted(true, "success"), false)
})
