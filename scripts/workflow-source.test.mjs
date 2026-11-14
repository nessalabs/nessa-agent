import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"

const code = [
  "scripts/workflow-source.mjs",
  "scripts/ci-pnpm-install.test.mjs",
  "scripts/desktop/measurement-plan.mjs",
  "scripts/desktop/measurement-plan.test.mjs",
]
const yaml = [
  ".github/actions/pnpm-install/action.yml",
  ".github/workflows/local-auth.yml",
  ".github/workflows/release.yml",
  ".github/workflows/conversation-stress-load.yml",
]

for (const [name, ending] of [
  ["LF", "\n"],
  ["CRLF", "\r\n"],
])
  test(`workflow contracts execute with ${name} checkout bytes`, () => {
    const root = mkdtempSync(join(tmpdir(), "nessa workflow checkout-"))
    try {
      for (const file of code) {
        const target = join(root, file)
        mkdirSync(dirname(target), { recursive: true })
        copyFileSync(new URL("../" + file, import.meta.url), target)
      }
      for (const file of yaml) {
        const source = readFileSync(new URL("../" + file, import.meta.url), "utf8")
        const checkout = source.replaceAll("\r\n", "\n").replaceAll("\n", ending)
        const target = join(root, file)
        mkdirSync(dirname(target), { recursive: true })
        writeFileSync(target, checkout)
        assert.equal(checkout.includes("\r\n"), name === "CRLF", file)
      }
      // This is a separate test-runner invocation, not another worker of this runner.
      const env = { ...process.env }
      delete env.NODE_TEST_CONTEXT
      const result = spawnSync(
        process.execPath,
        [
          "--test",
          "--test-reporter=tap",
          "scripts/ci-pnpm-install.test.mjs",
          "scripts/desktop/measurement-plan.test.mjs",
        ],
        { cwd: root, env, encoding: "utf8" },
      )
      assert.equal(result.error, undefined)
      assert.equal(result.signal, null)
      assert.equal(result.status, 0, result.stderr + result.stdout)
      assert.match(result.stdout, /# fail 0/)
      const passed = new Set(
        result.stdout.split(/\r?\n/).flatMap((line) => {
          const match = /^ok \d+ - (.+)$/.exec(line)
          return match ? [match[1]] : []
        }),
      )
      for (const contract of [
        "workflow installs go through the shared pnpm action",
        "manual Mac qualification retains the complete matrix and fixed check bound",
        "unknown, inherited and non-string scopes fail before publishing a plan",
        "the workflow consumes one validated plan before deciding builds, selector and artifact",
      ])
        assert.ok(passed.has(contract), contract)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

test("desktop checks include the workflow checkout regressions", () => {
  const checker = readFileSync(new URL("./check-desktop.mjs", import.meta.url), "utf8")
  assert.match(checker, /^ {2}"scripts\/workflow-source\.test\.mjs",\r?$/m)
})
