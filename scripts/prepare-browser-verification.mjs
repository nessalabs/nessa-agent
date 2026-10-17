// Manual CI only: prepare one same-repository source or integration tree.
import { execFileSync } from "node:child_process"
import { mkdirSync, writeFileSync } from "node:fs"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"

export function prepareBrowserVerification({
  cwd,
  source,
  base = "",
  evidence,
  repository,
}) {
  const git = (...args) =>
    execFileSync("git", args, { cwd, encoding: "utf8", stdio: "pipe" }).trim()
  const record = {
    repository,
    workflowCommit: git("rev-parse", "HEAD"),
    source,
    base: base || null,
    tree: null,
    state: "preparing",
  }
  mkdirSync(evidence, { recursive: true })
  const save = () =>
    writeFileSync(
      resolve(evidence, "provenance.json"),
      JSON.stringify(record, null, 2) + "\n",
    )
  save()
  try {
    for (const sha of base ? [source, base] : [source]) {
      if (!/^[0-9a-f]{40}$/.test(sha))
        throw new Error("Expected a full lowercase commit SHA")
      if (git("cat-file", "-t", sha) !== "commit") throw new Error("Expected a commit")
      // checkout@v4 fetch-depth: 0 supplies this repository's branch history.
      // A local object, tag or foreign PR ref alone does not establish membership.
      if (
        !git(
          "for-each-ref",
          "--format=%(refname)",
          `--contains=${sha}`,
          "refs/remotes/origin/",
        )
      )
        throw new Error("Commit is not reachable from a fetched repository branch")
    }
    if (git("status", "--porcelain")) throw new Error("Expected a clean CI checkout")
    git("checkout", "--detach", source)
    if (base)
      git(
        "-c",
        "user.name=Browser verification",
        "-c",
        "user.email=browser-verification@localhost",
        "merge",
        "--no-commit",
        "--no-ff",
        base,
      )
    record.tree = git("write-tree")
    record.state = "ready"
    save()
    return record
  } catch (error) {
    record.state = "failed"
    record.error = error.message
    save()
    throw error
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  prepareBrowserVerification({
    cwd: process.cwd(),
    source: process.env.SOURCE_SHA,
    base: process.env.BASE_SHA,
    evidence: process.env.BROWSER_EVIDENCE,
    repository: process.env.GITHUB_REPOSITORY,
  })
}
