import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("workflow installs go through the shared pnpm action", () => {
  const action = readFileSync(".github/actions/pnpm-install/action.yml", "utf8")
  assert.match(action, /pnpm install --frozen-lockfile --prefer-offline/)
  assert.match(action, /inputs\.save == 'true'/)
  assert.match(action, /steps\.cache\.outputs\.cache-hit != 'true'/)
  assert.match(action, /continue-on-error: true/)
  const steps = action.split("\n    - name: ").slice(1)
  const install = steps.find((step) => step.startsWith("Install from the lockfile"))
  const save = steps.find((step) => step.startsWith("Save the pnpm store"))
  assert.ok(install)
  assert.ok(save)
  assert.equal(install.includes("continue-on-error"), false)
  assert.match(save, /continue-on-error: true/)

  const localAuth = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const release = readFileSync(".github/workflows/release.yml", "utf8")
  for (const [path, text] of [
    [".github/workflows/local-auth.yml", localAuth],
    [".github/workflows/release.yml", release],
  ]) {
    assert.equal(text.includes("corepack enable"), false, path)
    assert.equal(text.includes("pnpm install --frozen-lockfile"), false, path)
  }
  const localSteps = localAuth.split("\n      - ").slice(1)
  const optional = localSteps.find((step) =>
    step.startsWith("name: Install optional desktop measurement dependencies"),
  )
  assert.ok(optional)
  assert.match(optional, /if: env\.NESSA_DESKTOP_PERFORMANCE == 'true'/)
  assert.match(optional, /uses: \.\/\.github\/actions\/pnpm-install/)
  assert.equal(optional.includes("save:"), false)
  assert.equal(
    localSteps.filter(
      (step) =>
        step !== optional && step.includes("uses: ./.github/actions/pnpm-install"),
    ).length,
    4,
  )
  assert.equal(
    localAuth.match(/save: \$\{\{ github\.ref == 'refs\/heads\/main' \}\}/g)?.length,
    2,
  )
  assert.equal(release.match(/uses: \.\/\.github\/actions\/pnpm-install/g)?.length, 2)
  assert.equal(
    release.match(/save: \$\{\{ github\.ref == 'refs\/heads\/main' \}\}/g)?.length,
    2,
  )

  const smoke = localAuth.slice(
    localAuth.indexOf("Install native desktop smoke dependencies"),
  )
  const step = smoke.slice(0, smoke.indexOf("\n      - "))
  assert.match(step, /uses: \.\/\.github\/actions\/pnpm-install/)
  assert.equal(step.includes("save:"), false)
})

test("desktop performance is manual, Mac-only, and keeps the existing budget", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.match(
    workflow,
    /desktop-performance:\n\s+description: [^\n]+\n\s+type: boolean\n\s+default: false/,
  )
  assert.match(
    workflow,
    /NESSA_DESKTOP_PERFORMANCE: \$\{\{ github\.event_name == 'workflow_dispatch' && inputs\['desktop-performance'\] && matrix\.os == 'macos-latest' \}\}/,
  )
  for (const name of [
    "Install optional desktop measurement dependencies",
    "Install the optional desktop measurement browser",
    "Record the desktop measurement host",
    "Measure the desktop frame budget on Mac",
  ]) {
    const step = workflow
      .split("\n      - ")
      .find((entry) => entry.startsWith(`name: ${name}\n`))
    assert.ok(step, name)
    assert.match(step, /if: env\.NESSA_DESKTOP_PERFORMANCE == 'true'/, name)
    assert.equal(step.includes("continue-on-error:"), false, name)
  }
  assert.match(
    workflow,
    /perf-budget\.mjs --mode prod --headed --engine chromium --layout columns,sidebar --channel bundled --runs 5 --out/,
  )
  assert.match(
    workflow,
    /name: Keep the desktop measurement in its job log\n\s+if: always\(\) && env\.NESSA_DESKTOP_PERFORMANCE == 'true'/,
  )
})
