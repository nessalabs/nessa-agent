// Real Git, synthetic repositories only; bare Node, no registry or host files.
import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"

import { prepareBrowserVerification } from "./prepare-browser-verification.mjs"

test("workflow evidence uses a sibling path accepted by artifact upload", () => {
  const workflow = readFileSync(
    new URL("../.github/workflows/canonical-browser-verification.yml", import.meta.url),
    "utf8",
  )
  const workspace = "/home/runner/work/repository/repository"
  const evidence = workflow
    .match(/^\s+BROWSER_EVIDENCE: (.+)$/m)[1]
    .replace("${{ github.workspace }}", workspace)
  // upload-artifact rejects literal dot segments even in absolute paths.
  assert.ok(!evidence.split("/").some((part) => part === "." || part === ".."))
  assert.equal(dirname(evidence), dirname(workspace))
  assert.notEqual(evidence, workspace)
})

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), "nessa-browser-ci-"))
  t.after(() => rmSync(cwd, { recursive: true, force: true }))
  const git = (...args) =>
    execFileSync("git", args, {
      cwd,
      encoding: "utf8",
      stdio: "pipe",
      env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" },
    }).trim()
  git("init", "--initial-branch=main")
  git("config", "user.name", "Synthetic fixture")
  git("config", "user.email", "fixture@localhost")
  const commit = (file, text) => {
    writeFileSync(join(cwd, file), text)
    git("add", file)
    git("commit", "-m", "fixture")
    return git("rev-parse", "HEAD")
  }
  const initial = commit("shared.txt", "initial\n")
  const base = commit("base.txt", "base\n")
  git("checkout", "-b", "source", initial)
  const source = commit("source.txt", "source\n")
  git("update-ref", "refs/remotes/origin/main", base)
  git("update-ref", "refs/remotes/origin/source", source)
  const evidence = join(cwd, "..", `${cwd.split("/").at(-1)}-evidence`)
  t.after(() => rmSync(evidence, { recursive: true, force: true }))
  const prepare = (overrides = {}) =>
    prepareBrowserVerification({
      cwd,
      source,
      base: "",
      evidence,
      repository: "fixture/repository",
      ...overrides,
    })
  const record = () => JSON.parse(readFileSync(join(evidence, "provenance.json"), "utf8"))
  return { cwd, git, source, base, prepare, record, commit }
}

test("source qualification preserves the requested commit and records its exact tree", (t) => {
  const f = fixture(t)
  const result = f.prepare()
  assert.equal(f.git("rev-parse", "HEAD"), f.source)
  assert.equal(result.tree, f.git("rev-parse", `${f.source}^{tree}`))
  assert.equal(result.source, f.source)
  assert.equal(result.base, null)
  assert.equal(result.state, "ready")
  assert.deepEqual(f.record(), result)
})

test("integration records the merged tree without creating a replacement source commit", (t) => {
  const f = fixture(t)
  const result = f.prepare({ base: f.base })
  assert.equal(f.git("rev-parse", "HEAD"), f.source)
  assert.equal(result.base, f.base)
  assert.equal(result.tree, f.git("write-tree"))
  assert.notEqual(result.tree, f.git("rev-parse", `${f.source}^{tree}`))
  assert.equal(readFileSync(join(f.cwd, "base.txt"), "utf8"), "base\n")
  assert.equal(readFileSync(join(f.cwd, "source.txt"), "utf8"), "source\n")
})

test("branch names, abbreviated SHAs and shell text are refused before checkout", (t) => {
  const f = fixture(t)
  for (const source of [
    "main",
    f.source.slice(0, 12),
    "$(touch injected)",
    "-".repeat(40),
  ]) {
    assert.throws(() => f.prepare({ source }), /full lowercase commit SHA/)
    assert.equal(f.git("rev-parse", "HEAD"), f.source)
    assert.equal(f.git("status", "--porcelain"), "")
    assert.equal(f.record().state, "failed")
  }
  assert.throws(() => f.prepare({ base: "main" }), /full lowercase commit SHA/)
})

test("local and tag-only commits cannot substitute for repository branch membership", (t) => {
  const f = fixture(t)
  const orphan = f.git(
    "commit-tree",
    f.git("rev-parse", "HEAD^{tree}"),
    "-m",
    "unreachable",
  )
  f.git("tag", "foreign-object", orphan)
  assert.throws(() => f.prepare({ source: orphan }), /not reachable/)
  assert.throws(() => f.prepare({ base: orphan }), /not reachable/)
  assert.equal(f.git("rev-parse", "HEAD"), f.source)
  assert.equal(f.record().tree, null)
})

test("a tree object is not accepted as a source commit", (t) => {
  const f = fixture(t)
  assert.throws(
    () => f.prepare({ source: f.git("rev-parse", "HEAD^{tree}") }),
    /Expected a commit/,
  )
})

test("dirty checkouts are refused without overwriting their files", (t) => {
  const f = fixture(t)
  writeFileSync(join(f.cwd, "shared.txt"), "retained\n")
  assert.throws(() => f.prepare(), /clean CI checkout/)
  assert.equal(readFileSync(join(f.cwd, "shared.txt"), "utf8"), "retained\n")
})

test("integration conflicts fail and preserve provenance with no successful tree claim", (t) => {
  const f = fixture(t)
  const source = f.commit("shared.txt", "source version\n")
  f.git("update-ref", "refs/remotes/origin/source", source)
  f.git("checkout", "--detach", f.base)
  const base = f.commit("shared.txt", "base version\n")
  f.git("update-ref", "refs/remotes/origin/main", base)
  assert.throws(() => f.prepare({ source, base }))
  assert.equal(f.record().state, "failed")
  assert.equal(f.record().tree, null)
  assert.equal(f.record().source, source)
  assert.equal(f.record().base, base)
  assert.equal(f.git("diff", "--name-only", "--diff-filter=U"), "shared.txt")
})
