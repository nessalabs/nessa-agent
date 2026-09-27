/**
 * What preflight refuses, and where it says it.
 *
 * It is the one script here that installs packages and builds a binary, so the
 * parts worth pinning are the ones that decide whether it does: an unknown
 * stage must stop before any of that, importing the module must do none of it,
 * and the progress it prints must stay off stdout, which belongs to whatever a
 * command produces for a machine to read.
 *
 * Driven as a child process rather than by importing and calling, because the
 * claim is about the script as something a person or a recipe runs.
 */
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { strict as assert } from "node:assert"
import { test } from "node:test"

const script = resolve(dirname(fileURLToPath(import.meta.url)), "preflight.mjs")

function run(args, env = {}) {
  return spawnSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...env },
  })
}

test("an unknown stage is refused, with the stages that exist", () => {
  const { status, stderr } = run(["nonsense"])
  assert.equal(status, 1)
  assert.match(stderr, /"nonsense" is not a stage/)
  assert.match(stderr, /gateway-ports\.json/)
})

test("a refusal names a command to run", () => {
  const { stderr } = run(["nonsense"])
  assert.match(stderr, /just start/)
})

test("nothing is installed or built on the way to refusing a stage", () => {
  // The refusal has to come first: the checks below it install packages into
  // the harness and build the gateway, and a typo in a stage name is not a
  // reason to do either.
  const { stdout, stderr } = run(["nonsense"])
  assert.doesNotMatch(`${stdout}${stderr}`, /npm ci|Compiling|building the gateway/)
})

test("progress is written to stderr, leaving stdout to the command", () => {
  const { stdout } = run(["nonsense"])
  assert.equal(stdout, "")
})

test("importing the module neither installs nor builds", async () => {
  // `main()` is guarded, so a caller that wants `checkAgent` alone — or a test
  // like this one — does not pay for a package install to get it.
  const before = Date.now()
  await import(script)
  assert.ok(Date.now() - before < 5000, "importing should not run a build")
})
