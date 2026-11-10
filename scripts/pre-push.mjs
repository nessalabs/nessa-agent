#!/usr/bin/env node
/**
 * The pre-push hook (`.githooks/pre-push`): runs the checks that
 * `pnpm check:changed --plan --git` selects, and refuses the push when one
 * fails.
 *
 * The checks read the checkout, but the push sends commits. So the hook runs
 * them only when the two are the same: every commit pushed is `HEAD`, and the
 * checkout has no change `HEAD` lacks, tracked or untracked. A new file left
 * out of the commit would otherwise pass here and fail in CI. Anything else is
 * refused with what to do. A push that only deletes refs checks nothing.
 * `git push --no-verify` skips the hook.
 *
 * Git writes one line per ref on stdin: `<local ref> <local sha> <remote ref>
 * <remote sha>`. A deleted ref has an all-zero local sha.
 */
import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { fileURLToPath, pathToFileURL } from "node:url"

import { runScripts } from "./check-changed.mjs"

const DELETED = /^0+$/

/** The local commits a push sends, from the lines git writes on stdin. */
export function pushedShas(text) {
  return text
    .split("\n")
    .map((line) => line.trim().split(/\s+/))
    .filter((fields) => fields.length === 4 && !DELETED.test(fields[1]))
    .map((fields) => fields[1])
}

/**
 * Whether the checkout is what the push sends.
 *
 * `commits` are the pushed objects peeled to commits (an annotated tag is its
 * own object), `head` is `HEAD`'s commit, and `status` is
 * `git status --porcelain` output.
 */
export function pushVerdict({ commits, head, status }) {
  if (commits.length === 0) return { run: false, reason: "the push only deletes refs" }
  const others = commits.filter((commit) => commit !== head)
  if (others.length > 0) {
    return {
      refuse: true,
      reason:
        `the push sends ${others.join(", ")}, which is not HEAD (${head}), and the checks ` +
        "read the checkout. Check out what you push, or push HEAD.",
    }
  }
  const changed = status.split("\n").filter((line) => line.trim().length > 0)
  if (changed.length > 0) {
    return {
      refuse: true,
      reason:
        "the checkout has changes the push leaves out, and the checks would read them:\n" +
        changed.map((line) => `  ${line}`).join("\n") +
        "\nCommit them, or put them aside with `git stash --include-untracked`.",
    }
  }
  return { run: true }
}

function git(args) {
  const result = spawnSync("git", args, { encoding: "utf8" })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr.trim()}`)
  return result.stdout
}

/** The plan `check:changed` makes for this branch against main. */
function changedPlan() {
  const script = fileURLToPath(new URL("./check-changed.mjs", import.meta.url))
  const result = spawnSync(process.execPath, [script, "--plan", "--git"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error("check:changed --plan --git failed")
  return JSON.parse(result.stdout)
}

export function main({
  stdin = readFileSync(0, "utf8"),
  run = git,
  makePlan = changedPlan,
  runChecks = runScripts,
  log = (line) => process.stderr.write(`${line}\n`),
} = {}) {
  const commits = pushedShas(stdin).map((sha) =>
    run(["rev-parse", `${sha}^{commit}`]).trim(),
  )
  const verdict = pushVerdict({
    commits,
    head: commits.length > 0 ? run(["rev-parse", "HEAD"]).trim() : "",
    status: commits.length > 0 ? run(["status", "--porcelain"]) : "",
  })
  if (verdict.refuse) {
    log(`pre-push: refused: ${verdict.reason}`)
    log("pre-push: `git push --no-verify` skips this hook.")
    return 1
  }
  if (!verdict.run) {
    log(`pre-push: nothing to check: ${verdict.reason}`)
    return 0
  }
  const plan = makePlan()
  log(`pre-push: ${plan.tier} plan: ${plan.reason}`)
  if (plan.commands.length === 0) return 0
  log(`pre-push: running ${plan.commands.map((command) => `pnpm ${command}`).join(", ")}`)
  const status = runChecks(plan.commands)
  if (status !== 0) log("pre-push: refused: a selected check failed")
  return status
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) process.exitCode = main()
