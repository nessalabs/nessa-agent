import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const repo = fileURLToPath(new URL("..", import.meta.url))
const script = fileURLToPath(
  new URL("../scripts/check-sdk-domain-coverage.sh", import.meta.url),
)

function fakeCargo(marker) {
  const directory = mkdtempSync(path.join(tmpdir(), "coverage-cargo-"))
  writeFileSync(
    path.join(directory, "cargo"),
    `#!/bin/sh\nprintf '%s\\n' "$CARGO_TARGET_DIR" > "$MARKER"\n`,
    { mode: 0o755 },
  )
  return {
    directory,
    env: {
      ...process.env,
      MARKER: marker,
      PATH: `${directory}${path.delimiter}${process.env.PATH}`,
    },
  }
}

test("the domain gate keeps its serial threads and 100% thresholds", () => {
  const source = readFileSync(script, "utf8")
  assert.match(source, /: "\$\{RUST_TEST_THREADS:=1\}"/)
  assert.match(source, /export RUST_TEST_THREADS/)
  assert.match(source, /--fail-under-lines 100/)
  assert.match(source, /--fail-under-functions 100/)
  assert.match(source, /--fail-under-regions 100/)
  assert.match(source, /--skip link_attack_tests/)
  assert.match(source, /-p nessa-sdk/)
})

const workspaceTarget = path.join(repo, "target")

/** Run the coverage script with a stand-in `cargo` that records `CARGO_TARGET_DIR`. */
function runCoverage(cargo, target) {
  return spawnSync("bash", [script], {
    cwd: repo,
    env: { ...cargo.env, NESSA_SDK_COVERAGE_TARGET: target },
    encoding: "utf8",
  })
}

/**
 * Two directories a coverage run can be pointed at. One path is already
 * canonical. The other reaches its directory through a symlink, which is how
 * macOS spells `tmpdir()` (`/var` → `/private/var`). Linux CI has no such
 * symlink, so the test builds one.
 */
function coverageTargetSpellings() {
  const canonical = realpathSync(mkdtempSync(path.join(tmpdir(), "coverage-canon-")))
  const real = realpathSync(mkdtempSync(path.join(tmpdir(), "coverage-real-")))
  const linkParent = mkdtempSync(path.join(tmpdir(), "coverage-alias-"))
  const link = path.join(linkParent, "var")
  symlinkSync(real, link)
  const aliased = mkdtempSync(path.join(link, "coverage-target-"))
  return {
    targets: [
      { kind: "canonical", directory: canonical },
      { kind: "aliased", directory: aliased },
    ],
    remove() {
      rmSync(canonical, { recursive: true, force: true })
      rmSync(linkParent, { recursive: true, force: true })
      rmSync(real, { recursive: true, force: true })
    },
  }
}

test("a local run uses a temporary target and removes only that directory", () => {
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-temp`)
  rmSync(marker, { force: true })
  const cargo = fakeCargo(marker)
  let target = ""
  try {
    const result = runCoverage(cargo, "")
    assert.equal(result.status, 0, result.stderr)
    target = readFileSync(marker, "utf8").trim()
    assert.match(target, /nessa-sdk-domain-coverage\./)
    assert.equal(path.basename(path.dirname(target)) === "target", false)
    assert.notEqual(target, workspaceTarget)
    assert.equal(spawnSync("test", ["!", "-e", target]).status, 0)
  } finally {
    if (target.includes(`${path.sep}nessa-sdk-domain-coverage.`)) {
      rmSync(target, { recursive: true, force: true })
    }
    rmSync(cargo.directory, { recursive: true, force: true })
    rmSync(marker, { force: true })
  }
})

test("a named coverage target is kept and is not the workspace target", () => {
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-kept`)
  rmSync(marker, { force: true })
  const cargo = fakeCargo(marker)
  const spellings = coverageTargetSpellings()
  try {
    for (const { kind, directory } of spellings.targets) {
      rmSync(marker, { force: true })
      const result = runCoverage(cargo, directory)
      assert.equal(result.status, 0, result.stderr)
      const emitted = readFileSync(marker, "utf8").trim()
      assert.equal(realpathSync(emitted), realpathSync(directory), `${kind}: ${emitted}`)
      assert.notEqual(realpathSync(emitted), workspaceTarget, kind)
      assert.equal(spawnSync("test", ["-d", directory]).status, 0, kind)
    }
  } finally {
    spellings.remove()
    rmSync(cargo.directory, { recursive: true, force: true })
    rmSync(marker, { force: true })
  }
})

test("the workspace target directory is refused before cargo runs", () => {
  const existed = spawnSync("test", ["-d", workspaceTarget]).status === 0
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-refuse`)
  rmSync(marker, { force: true })
  const cargo = fakeCargo(marker)
  const linkParent = mkdtempSync(path.join(tmpdir(), "coverage-workspace-alias-"))
  const alias = path.join(linkParent, "target")
  try {
    const spelled = runCoverage(cargo, workspaceTarget)
    assert.equal(spelled.status, 1)
    assert.match(spelled.stderr, /refuses the workspace target directory/)
    assert.equal(spawnSync("test", ["!", "-e", marker]).status, 0)

    // A dangling symlink is not a directory, so the alias is created only once
    // the workspace target exists. The script then refuses that same directory.
    if (spawnSync("test", ["-d", workspaceTarget]).status !== 0) {
      mkdirSync(workspaceTarget)
    }
    symlinkSync(workspaceTarget, alias)
    rmSync(marker, { force: true })
    const aliased = runCoverage(cargo, alias)
    assert.equal(aliased.status, 1, aliased.stderr)
    assert.match(aliased.stderr, /refuses the workspace target directory/)
    assert.equal(spawnSync("test", ["!", "-e", marker]).status, 0)
    assert.equal(spawnSync("test", ["-d", workspaceTarget]).status, 0)
  } finally {
    rmSync(linkParent, { recursive: true, force: true })
    if (!existed) rmSync(workspaceTarget, { recursive: true, force: true })
    rmSync(cargo.directory, { recursive: true, force: true })
    rmSync(marker, { force: true })
  }
})
