import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { hooksAction, HOOKS } from "./install-git-hooks.mjs"
import { main, pushedShas, pushVerdict } from "./pre-push.mjs"

const A = "a".repeat(40)
const B = "b".repeat(40)
const ZERO = "0".repeat(40)

test("the pushed commits are the non-deleted local shas git writes", () => {
  const stdin = [
    `refs/heads/topic ${A} refs/heads/topic ${ZERO}`,
    `(delete) ${ZERO} refs/heads/old ${B}`,
    "",
  ].join("\n")
  assert.deepEqual(pushedShas(stdin), [A])
  assert.deepEqual(pushedShas(""), [])
})

test("a push of a clean HEAD runs the checks", () => {
  assert.deepEqual(pushVerdict({ commits: [A, A], head: A, status: "" }), { run: true })
})

test("a push that only deletes refs checks nothing", () => {
  assert.equal(pushVerdict({ commits: [], head: A, status: "" }).run, false)
})

test("a push of a commit that is not HEAD is refused, since the checks read the checkout", () => {
  const verdict = pushVerdict({ commits: [A, B], head: A, status: "" })
  assert.equal(verdict.refuse, true)
  assert.match(verdict.reason, new RegExp(B))
})

test("a checkout with changes the push leaves out is refused, untracked files included", () => {
  for (const status of [" M src/a.ts\n", "?? src/new.ts\n"]) {
    const verdict = pushVerdict({ commits: [A], head: A, status })
    assert.equal(verdict.refuse, true)
    assert.match(verdict.reason, /stash --include-untracked/)
  }
})

/** `main` with git answered from a table, recording what it ran. */
function hook({
  status = "",
  plan,
  checkStatus = 0,
  stdin = `refs/heads/t ${A} refs/heads/t ${ZERO}\n`,
}) {
  const ran = []
  const lines = []
  const code = main({
    stdin,
    run: (args) => {
      if (args[0] === "rev-parse") return `${A}\n`
      if (args[0] === "status") return status
      throw new Error(`unexpected git ${args.join(" ")}`)
    },
    makePlan: () => plan,
    runChecks: (commands) => {
      ran.push(...commands)
      return checkStatus
    },
    log: (line) => lines.push(line),
  })
  return { code, ran, lines }
}

test("the hook runs exactly what the plan selects and refuses when one fails", () => {
  const plan = {
    tier: "narrow",
    commands: ["frontend:check", "sdk:check"],
    reason: "narrowed",
  }
  const passed = hook({ plan })
  assert.equal(passed.code, 0)
  assert.deepEqual(passed.ran, plan.commands)
  const failed = hook({ plan, checkStatus: 2 })
  assert.equal(failed.code, 2)
  assert.match(failed.lines.at(-1), /refused: a selected check failed/)
})

test("a docs-only plan selects nothing and the push goes on", () => {
  const result = hook({
    plan: { tier: "documentation", commands: [], reason: "Markdown" },
  })
  assert.equal(result.code, 0)
  assert.deepEqual(result.ran, [])
})

test("a refused checkout never makes a plan", () => {
  const result = hook({
    status: "?? scratch.txt\n",
    plan: {
      get commands() {
        throw new Error("planned")
      },
    },
  })
  assert.equal(result.code, 1)
  assert.deepEqual(result.ran, [])
})

test("pnpm install turns the hooks on only where nothing else set them", () => {
  assert.deepEqual(hooksAction({ inCheckout: false, hooksPath: null }), { set: false })
  assert.deepEqual(hooksAction({ inCheckout: true, hooksPath: null }), { set: true })
  assert.deepEqual(hooksAction({ inCheckout: true, hooksPath: HOOKS }), { set: false })
  const own = hooksAction({ inCheckout: true, hooksPath: "/home/me/hooks" })
  assert.equal(own.set, false)
  assert.match(own.note, /\/home\/me\/hooks/)
})

test("git push runs the checked-in hook: a failing check or a dirty checkout is refused", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-pre-push-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const work = join(root, "work")
  const remote = join(root, "remote.git")
  const git = (...args) =>
    execFileSync("git", args, { cwd: work, stdio: "pipe", encoding: "utf8" })
  const write = (path, text) => {
    const file = join(work, path)
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, text)
  }
  const here = dirname(fileURLToPath(import.meta.url))
  mkdirSync(work)
  execFileSync("git", ["init", "--bare", "--initial-branch=main", remote], {
    stdio: "pipe",
  })
  git("init", "--initial-branch=main")
  git("config", "user.name", "Pre-push fixture")
  git("config", "user.email", "pre-push@localhost")
  for (const name of ["pre-push.mjs", "check-changed.mjs", "documentation-only.mjs"]) {
    mkdirSync(join(work, "scripts"), { recursive: true })
    copyFileSync(join(here, name), join(work, "scripts", name))
  }
  mkdirSync(join(work, HOOKS))
  copyFileSync(join(here, "..", HOOKS, "pre-push"), join(work, HOOKS, "pre-push"))
  chmodSync(join(work, HOOKS, "pre-push"), 0o755)
  // One check in the chain, which fails while `fail` exists.
  write(
    "package.json",
    JSON.stringify({
      scripts: {
        check: "pnpm frontend:check",
        "frontend:check":
          "node -e \"process.exit(require('fs').existsSync('src/fail') ? 3 : 0)\"",
      },
    }),
  )
  write("src/a.ts", "export {}\n")
  git("add", ".")
  git("-c", "commit.gpgsign=false", "commit", "-m", "base")
  git("remote", "add", "origin", remote)
  git("push", "--no-verify", "origin", "main")
  git("fetch", "origin")
  git("config", "core.hooksPath", HOOKS)
  git("checkout", "-b", "topic")
  const push = () =>
    spawnSync("git", ["push", "origin", "topic"], { cwd: work, encoding: "utf8" })

  write("README.md", "docs\n")
  git("add", ".")
  git("-c", "commit.gpgsign=false", "commit", "-m", "docs")
  const docs = push()
  assert.equal(docs.status, 0, docs.stderr)
  assert.doesNotMatch(docs.stderr, /running pnpm/)

  write("src/a.ts", "export const a = 1\n")
  write("src/fail", "")
  git("add", "src")
  git("-c", "commit.gpgsign=false", "commit", "-m", "failing check")
  const failing = push()
  assert.notEqual(failing.status, 0)
  assert.match(failing.stderr, /running pnpm frontend:check/)
  assert.match(failing.stderr, /refused: a selected check failed/)

  git("rm", "-q", "src/fail")
  write("scratch.txt", "left out\n")
  git("-c", "commit.gpgsign=false", "commit", "-m", "fixed")
  const dirty = push()
  assert.notEqual(dirty.status, 0)
  assert.match(dirty.stderr, /\?\? scratch\.txt/)

  rmSync(join(work, "scratch.txt"))
  const clean = push()
  assert.equal(clean.status, 0, clean.stderr)
  assert.match(clean.stderr, /running pnpm frontend:check/)
})
