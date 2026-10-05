import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
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

test("a local run uses a temporary target and removes only that directory", () => {
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-temp`)
  rmSync(marker, { force: true })
  const cargo = fakeCargo(marker)
  const result = spawnSync("bash", [script], {
    cwd: repo,
    env: { ...cargo.env, NESSA_SDK_COVERAGE_TARGET: "" },
    encoding: "utf8",
  })
  assert.equal(result.status, 0, result.stderr)
  const target = readFileSync(marker, "utf8").trim()
  assert.match(target, /nessa-sdk-domain-coverage\./)
  assert.equal(path.basename(path.dirname(target)) === "target", false)
  assert.notEqual(target, path.join(repo, "target"))
  assert.equal(spawnSync("test", ["!", "-e", target]).status, 0)
  rmSync(cargo.directory, { recursive: true, force: true })
  rmSync(marker, { force: true })
})

test("a named coverage target is kept and is not the workspace target", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "coverage-target-"))
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-kept`)
  const cargo = fakeCargo(marker)
  const result = spawnSync("bash", [script], {
    cwd: repo,
    env: { ...cargo.env, NESSA_SDK_COVERAGE_TARGET: directory },
    encoding: "utf8",
  })
  assert.equal(result.status, 0, result.stderr)
  assert.equal(readFileSync(marker, "utf8").trim(), directory)
  assert.equal(spawnSync("test", ["-d", directory]).status, 0)
  rmSync(directory, { recursive: true, force: true })
  rmSync(cargo.directory, { recursive: true, force: true })
  rmSync(marker, { force: true })
})

test("the workspace target directory is refused before cargo runs", () => {
  const workspaceTarget = path.join(repo, "target")
  const existed = spawnSync("test", ["-d", workspaceTarget]).status === 0
  const marker = path.join(tmpdir(), `coverage-marker-${process.pid}-refuse`)
  rmSync(marker, { force: true })
  const cargo = fakeCargo(marker)
  const result = spawnSync("bash", [script], {
    cwd: repo,
    env: { ...cargo.env, NESSA_SDK_COVERAGE_TARGET: workspaceTarget },
    encoding: "utf8",
  })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /refuses the workspace target directory/)
  assert.equal(spawnSync("test", ["!", "-e", marker]).status, 0)
  if (!existed) rmSync(workspaceTarget, { recursive: true, force: true })
  rmSync(cargo.directory, { recursive: true, force: true })
  rmSync(marker, { force: true })
})
