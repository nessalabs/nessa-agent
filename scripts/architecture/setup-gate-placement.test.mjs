import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { setupGatePlacementViolations } from "./setup-gate-placement.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const PATH = "src/composition/browser.tsx"

const composed = `
  return (
    <BrowserSetupGate dependencies={scope.dependencies}>
      <SessionLifecycle dependencies={scope.dependencies} />
      <App attachmentResources={scope.dependencies.attachments} />
    </BrowserSetupGate>
  )
`

test("a composition that puts the panel inside the gate passes", () => {
  assert.deepEqual(setupGatePlacementViolations(PATH, composed), [])
})

test("a composition that renders the panel on its own is rejected", () => {
  // The deletion that leaves every suite green: the gate goes, the panel comes
  // up, and the agent the user picked reaches nothing.
  const violations = setupGatePlacementViolations(
    PATH,
    `
  return (
    <>
      <SessionLifecycle dependencies={scope.dependencies} />
      <App attachmentResources={scope.dependencies.attachments} />
    </>
  )
`,
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /must render <BrowserSetupGate>/)
})

test("a gate rendered beside the panel rather than around it is rejected", () => {
  const violations = setupGatePlacementViolations(
    PATH,
    `
  return (
    <>
      <BrowserSetupGate dependencies={scope.dependencies}></BrowserSetupGate>
      <App attachmentResources={scope.dependencies.attachments} />
    </>
  )
`,
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /inside <BrowserSetupGate>/)
})

test("a gate given nothing to hand the choice to is rejected", () => {
  const violations = setupGatePlacementViolations(
    PATH,
    composed.replace(" dependencies={scope.dependencies}", ""),
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /keep the chosen agent/)
})

test("the composition in this repository satisfies the rule", () => {
  assert.deepEqual(
    setupGatePlacementViolations(PATH, readFileSync(join(root, PATH), "utf8")),
    [],
  )
})

test("any other file is not this rule's business", () => {
  assert.deepEqual(
    setupGatePlacementViolations("src/composition/main.tsx", "<App />"),
    [],
  )
})
