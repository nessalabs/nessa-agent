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
  assert.equal(
    localSteps.filter((step) => step.includes("uses: ./.github/actions/pnpm-install"))
      .length,
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

function loadSteps() {
  return readFileSync(".github/workflows/conversation-stress-load.yml", "utf8").split(
    "\n      - ",
  )
}

function named(steps, name) {
  const step = steps.find((entry) => entry.startsWith(`name: ${name}\n`))
  assert.ok(step, name)
  return step
}

test("desktop qualification is an explicit mode of the existing load job", () => {
  const workflow = readFileSync(".github/workflows/conversation-stress-load.yml", "utf8")
  const native = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.match(
    workflow,
    /desktop-performance:\n\s+description: [^\n]+\n\s+type: boolean\n\s+default: false/,
  )
  assert.match(
    workflow,
    /name: \$\{\{ inputs\['desktop-performance'\] && 'Desktop browser qualification \(macos-latest\)' \|\| 'Conversation stress at load' \}\}/,
  )
  assert.match(
    workflow,
    /runs-on: \$\{\{ inputs\['desktop-performance'\] && 'macos-latest' \|\| 'ubuntu-latest' \}\}/,
  )
  assert.match(workflow, /timeout-minutes: 210/)
  assert.match(workflow, /cron: "23 4 \* \* \*"/)
  const jobs = workflow.slice(workflow.indexOf("\njobs:\n"))
  assert.deepEqual(jobs.match(/^ {2}[a-z-]+:/gm), ["  load:"])
  assert.doesNotMatch(
    native,
    /desktop-performance|NESSA_DESKTOP_PERFORMANCE|Keep desktop result receipt/,
  )
  assert.match(native, /timeout-minutes: 140/)
})

test("manual Mac qualification retains the complete matrix and fixed check bound", () => {
  const steps = loadSteps()
  const browser = named(steps, "Install the desktop qualification browsers")
  assert.match(browser, /if: \$\{\{ inputs\['desktop-performance'\] \}\}/)
  assert.match(browser, /run: pnpm exec playwright install chromium webkit/)
  assert.match(browser, /timeout-minutes: 15/)
  const matrix = named(steps, "Measure the complete desktop matrix on Mac")
  assert.match(matrix, /if: \$\{\{ inputs\['desktop-performance'\] \}\}/)
  assert.match(
    matrix,
    /run-all\.mjs --headed --layout columns,sidebar --channel bundled --runs 5 --out "\$STRESS_EVIDENCE\/desktop-performance\.json"/,
  )
  assert.match(matrix, /timeout-minutes: 135/)
  for (const forbidden of [
    "continue-on-error",
    "--quick",
    "--only",
    "--skip",
    "--engine",
    "tee ",
    "2>&1",
  ])
    assert.equal(matrix.includes(forbidden), false, forbidden)
  const workflow = steps.join("\n      - ")
  assert.ok(
    workflow.indexOf("run: cargo build -p nessa-server -p nessa-mcp") <
      workflow.indexOf("name: Record the desktop measurement host"),
  )
  assert.ok(
    workflow.indexOf("name: Record the desktop measurement host") <
      workflow.indexOf("name: Measure the complete desktop matrix on Mac"),
  )
  const host = named(steps, "Record the desktop measurement host")
  assert.match(host, /if: \$\{\{ inputs\['desktop-performance'\] \}\}/)
  assert.match(host, /desktop-host\.json/)
  assert.match(host, /loadAverage: loadavg\(\)/)
  assert.match(host, /SystemInfo\.getInfo/)
  assert.match(host, /commit: process\.env\.GITHUB_SHA/)
  assert.match(host, /finally \{ await browser\.close\(\) \}/)
})

test("default and scheduled load keep Ubuntu workloads, seeds, and failure-time chaos", () => {
  const steps = loadSteps()
  const workflow = steps.join("\n      - ")
  const globalEnv = workflow.slice(
    workflow.indexOf("    env:"),
    workflow.indexOf("    steps:"),
  )
  assert.match(globalEnv, /STRESS_EVIDENCE:/)
  assert.doesNotMatch(globalEnv, /NESSA_STRESS_|NESSA_CHAOS_/)
  const secret = steps.find((step) =>
    step.startsWith("uses: ./.github/actions/secret-service\n"),
  )
  assert.match(secret, /if: runner\.os == 'Linux'/)
  assert.match(
    workflow,
    /shared-key: \$\{\{ inputs\['desktop-performance'\] && 'local-auth' \|\| 'conversation-stress' \}\}/,
  )
  assert.match(workflow, /save-if: false/)
  assert.doesNotMatch(workflow, /save-if: true|save: true/)
  const install = named(steps, "Install the pinned Chromium and its runtime dependencies")
  assert.match(install, /if: \$\{\{ !inputs\['desktop-performance'\] \}\}/)
  assert.match(install, /run: pnpm exec playwright install --with-deps chromium/)
  assert.match(install, /timeout-minutes: 15/)
  const seed = named(steps, "Pick the seed")
  assert.match(seed, /if: \$\{\{ !inputs\['desktop-performance'\] \}\}/)
  assert.match(seed, /NESSA_STRESS_SEED=\$seed/)
  assert.match(seed, /NESSA_CHAOS_SEED=\$seed/)
  const stress = named(steps, "Run the conversation stress check at load")
  assert.match(stress, /if: \$\{\{ !inputs\['desktop-performance'\] \}\}/)
  assert.match(
    stress,
    /pnpm test:e2e:stress -- --channel bundled --evidence "\$STRESS_EVIDENCE"/,
  )
  assert.match(stress, /timeout-minutes: 100/)
  assert.match(stress, /NESSA_STRESS_ROUNDS: \$\{\{ inputs\.rounds \|\| '80' \}\}/)
  assert.match(stress, /NESSA_STRESS_PARALLEL: \$\{\{ inputs\.parallel \|\| '12' \}\}/)
  assert.match(stress, /NESSA_STRESS_REVIEWS: \$\{\{ inputs\.reviews \|\| '64' \}\}/)
  assert.doesNotMatch(stress, /NESSA_CHAOS_/)
  const chaos = named(steps, "Run the conversation chaos check at load")
  assert.match(
    chaos,
    /if: \$\{\{ !cancelled\(\) && !inputs\['desktop-performance'\] \}\}/,
  )
  assert.match(chaos, /pnpm test:e2e:chaos -- --evidence "\$STRESS_EVIDENCE\/chaos"/)
  assert.match(chaos, /timeout-minutes: 60/)
  assert.match(chaos, /NESSA_CHAOS_RUNS: \$\{\{ inputs\.chaos-runs \|\| '8' \}\}/)
  assert.match(chaos, /NESSA_CHAOS_WAVES: \$\{\{ inputs\.chaos-waves \|\| '150' \}\}/)
  assert.doesNotMatch(chaos, /NESSA_STRESS_/)
})

test("desktop failures preserve source-bound structured receipts without a log workaround", () => {
  const steps = loadSteps()
  const log = named(steps, "Keep the desktop measurement in its job log")
  assert.match(log, /if: always\(\) && inputs\['desktop-performance'\]/)
  assert.match(log, /cat "\$STRESS_EVIDENCE\/desktop-performance\.json"/)
  assert.match(
    log,
    /ci-receipt\.mjs "\$STRESS_EVIDENCE\/desktop-performance\.json" manifest/,
  )
  for (let group = 1; group <= 3; group++) {
    const receipt = named(steps, `Keep desktop result receipt ${group}`)
    assert.match(receipt, /if: always\(\) && inputs\['desktop-performance'\]/)
    assert.ok(
      receipt.includes(
        `ci-receipt.mjs "$STRESS_EVIDENCE/desktop-performance.json" group${group}`,
      ),
    )
    assert.doesNotMatch(receipt, /continue-on-error/)
  }
  const artifact = named(steps, "Keep the evidence")
  assert.match(artifact, /if: always\(\)/)
  assert.match(artifact, /path: \$\{\{ env\.STRESS_EVIDENCE \}\}/)
  assert.match(
    artifact,
    /inputs\['desktop-performance'\] && 'desktop-browser-qualification' \|\| 'conversation-stress-load'/,
  )
  assert.match(artifact, /inputs\['desktop-performance'\] && 'error' \|\| 'ignore'/)
  assert.match(artifact, /retention-days: 14/)
})
