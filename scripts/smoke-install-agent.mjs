/** The real `nessa install-agent` process, and what it puts on each stream.
 *
 * `nessa install-agent NAME` is documented as reporting what it installed "as
 * JSON on stdout", which makes stdout an interface the desktop can parse rather
 * than output a person reads. Nothing in the unit tests holds the *process* to
 * that. They call `write_report` with a writer of their own, so a tracing layer
 * pointed at stdout, a `println!` left in a failure path, or a progress line
 * would all leave them green and break every reader at once.
 *
 * So this runs the binary and reads its streams. Every case asserts the same
 * rule from a different side: stdout carries the report or it carries nothing.
 *
 * Isolated in a temporary data root, like the auth smoke test beside it, so it
 * neither reads nor writes the runtimes this machine actually has.
 */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"

const root = fileURLToPath(new URL("../", import.meta.url))
const directory = mkdtempSync(join(tmpdir(), "nessa-install-e2e-"))
const binary = join(
  root,
  process.platform === "win32" ? "target/debug/nessa.exe" : "target/debug/nessa",
)
const env = {
  ...process.env,
  NESSA_DATA_DIR: join(directory, "data"),
  NESSA_INSTANCE: "install-e2e",
  NESSA_STAGE: "ci",
}

/** The command, run for real, with its streams kept apart. */
function install(...args) {
  return spawnSync(binary, ["install-agent", ...args], { env, encoding: "utf8" })
}

/** The report, held to being exactly one line of JSON and nothing else. */
function report(run) {
  assert.equal(run.status, 0, `install failed: ${run.stderr}`)
  assert.ok(
    run.stdout.endsWith("\n"),
    `the report is not a line: ${JSON.stringify(run.stdout)}`,
  )
  assert.equal(
    run.stdout.trimEnd().split("\n").length,
    1,
    `stdout is more than the report: ${JSON.stringify(run.stdout)}`,
  )
  return JSON.parse(run.stdout)
}

/** Why this run cannot reach a report, when that is not this build's fault.
 *
 * Two reasons, and neither is a defect. A runner Nessa pins no build for —
 * Windows today — has nothing to install, and a machine with no route to the
 * registry has nothing to install it from. Everything else is a failure and is
 * reported as one: a digest that did not match, an archive with no executable
 * in it, a store that could not be written are faults of this build however
 * little network there was.
 *
 * Returns the reason to say out loud, or `undefined` when a report was owed. */
function unavailable(stderr) {
  if (/no tested opencode release/.test(stderr)) {
    return `nessa pins no opencode build for ${process.platform}-${process.arch}`
  }
  if (stderr.includes("nothing was installed, try again")) {
    return "the pinned archive could not be fetched"
  }
  return undefined
}

try {
  // An agent Nessa pins nothing for. The refusal names it, and the one thing
  // that must not happen is a reader being handed a line that is not a report.
  const unknown = install("claude")
  assert.equal(unknown.status, 25, `expected the agent exit code: ${unknown.stderr}`)
  assert.equal(unknown.stdout, "", `a refusal wrote to stdout: ${unknown.stdout}`)
  assert.match(unknown.stderr, /claude is not an agent nessa installs/)

  // A command line this binary cannot run at all. Refused before anything is
  // dispatched, so stdout is untouched for a second reason, and the exit code
  // is the usage one rather than the installer's.
  const missing = install()
  assert.notEqual(missing.status, 0)
  assert.equal(missing.stdout, "", `a usage refusal wrote to stdout: ${missing.stdout}`)
  assert.match(missing.stderr, /install-agent takes one agent name/)

  // A name no agent could have. Refused by the parser for the same reason.
  const hostile = install("../opencode")
  assert.notEqual(hostile.status, 0)
  assert.equal(hostile.stdout, "", `a rejected name wrote to stdout: ${hostile.stdout}`)

  // And the case the contract exists for: a real install, of the real pinned
  // archive, verified against the real compiled-in digest.
  //
  // Skipped rather than failed when the archive cannot be fetched, because a
  // machine with no route to the registry is not a broken build and this must
  // not become the check that goes red when a CDN blinks. Skipped loudly: what
  // was not checked is said, so a run that quietly stopped testing the thing
  // it is named after cannot pass for one that did.
  const installed = install("opencode")
  const skipped = installed.status === 0 ? undefined : unavailable(installed.stderr)
  if (skipped) {
    console.log(
      `install-agent e2e passed the refusal cases; the report itself was not checked because ${skipped}`,
    )
  } else {
    const first = report(installed)
    assert.equal(first.agent, "opencode")
    assert.match(first.version, /^\d+\.\d+\.\d+/)
    assert.ok(first.executable.startsWith(env.NESSA_DATA_DIR), first.executable)
    assert.equal(first.downloaded, true)

    // Again, over the install that is now there. The same report, except for
    // the one field that says a hundred megabytes did not move — which is the
    // only way a surface can tell the two apart, and the field a store that
    // silently reinstalled would get wrong.
    const second = report(install("opencode"))
    assert.deepEqual(second, { ...first, downloaded: false })

    console.log(
      `install-agent e2e passed: refusals keep stdout clean, and opencode ${first.version} installed and reported one line of json, then reported itself already present`,
    )
  }
} finally {
  rmSync(directory, { recursive: true, force: true })
}
