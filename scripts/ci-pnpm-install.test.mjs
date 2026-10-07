import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("workflow installs go through the shared pnpm action", () => {
  const action = readFileSync(".github/actions/pnpm-install/action.yml", "utf8")
  assert.match(action, /pnpm install --frozen-lockfile --prefer-offline/)
  assert.match(action, /inputs\.save == 'true'/)
  assert.match(action, /steps\.cache\.outputs\.cache-hit != 'true'/)
  assert.match(action, /continue-on-error: true/)

  const localAuth = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const release = readFileSync(".github/workflows/release.yml", "utf8")
  for (const [path, text] of [
    [".github/workflows/local-auth.yml", localAuth],
    [".github/workflows/release.yml", release],
  ]) {
    assert.equal(text.includes("corepack enable"), false, path)
    assert.equal(text.includes("pnpm install --frozen-lockfile"), false, path)
  }
  assert.equal(localAuth.match(/uses: \.\/\.github\/actions\/pnpm-install/g)?.length, 3)
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
