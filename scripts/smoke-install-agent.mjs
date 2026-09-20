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

/** Whether Nessa pins an Opencode build for the machine this is running on.
 *
 * Read from the platform, deliberately, and never from what the command said.
 * `install-agent` has two refusals that both open "nessa has no tested
 * opencode release": one for a platform with no pins at all, and one for a
 * platform that has them where none of the builds runs on *this* machine. On
 * Windows the first is the truth. On a runner Nessa does pin for, either
 * sentence can only mean a regression — a pin dropped from
 * `agent-releases.json`, `host_libc` or `host_has_avx2` answering wrongly,
 * `satisfies` broken — and a check that read it as "nothing to install here"
 * would turn every one of those into a green step that verified nothing, with
 * the digest never compared against the registry.
 *
 * So there is one platform this skips on, it is named here rather than
 * inferred, and it is the only one. */
const PINNED = process.platform !== "win32"

/** Whether a refusal is this machine having no route to the registry.
 *
 * Exactly `SourceFailure::Unreachable`, matched on its own words rather than
 * on the retry advice `explain` appends. The advice is wider than the fault:
 * every `Download` failure that is not a refused status or an oversized body
 * carries it, and that includes `NotStored` — the bytes arrived and this
 * machine could not keep them, which is a full disk here and not a quiet
 * network. Skipping on the advice would skip on a store that could not be
 * written, which this file says in the same breath is a failure. */
function unreachable(stderr) {
  return stderr.includes("could not reach the release archive")
}

/** What each failure `install-agent` can report must do to the skip above.
 *
 * Held here rather than left to be read, because the fault this guards is a
 * skip condition quietly growing wider than the one thing it is for, and the
 * run where that matters is the run where it stops checking anything. These
 * are the `SourceFailure` and `InstallFailure` sentences as `explain` emits
 * them, retry advice included — the advice is the part that is wider than the
 * fault, so it is present in the ones that must not skip.
 */
const SKIP_CASES = [
  [
    "could not reach the release archive: connection reset; nothing was installed, try again",
    true,
  ],
  [
    "could not store the release archive: No space left on device; nothing was installed, try again",
    false,
  ],
  [
    "the release archive was refused with status 404; the pinned release may have been withdrawn (404)",
    false,
  ],
  [
    "the archive is not the pinned one; nothing was installed and this is not worth retrying",
    false,
  ],
]

for (const [message, skips] of SKIP_CASES) {
  assert.equal(
    unreachable(message),
    skips,
    `${skips ? "must" : "must not"} be treated as an unreachable registry: ${message}`,
  )
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
  const installed = install("opencode")
  if (!PINNED) {
    // Nothing is pinned here, so the refusal is the whole of what this
    // platform can be held to — and it is held to it rather than skipped past,
    // because "nessa installs nothing on Windows" is itself a contract and a
    // Windows pin arriving without the rest of this working should not be
    // silent.
    assert.notEqual(installed.status, 0, `opencode installed on ${process.platform}`)
    assert.match(installed.stderr, /no tested opencode release/)
    assert.equal(installed.stdout, "", `a refusal wrote to stdout: ${installed.stdout}`)
    console.log(
      `install-agent e2e passed the refusal cases, including that nessa pins no opencode build for ${process.platform}; there was nothing to install, so the report itself was not checked`,
    )
  } else if (installed.status !== 0 && unreachable(installed.stderr)) {
    // The one skip on a platform that is pinned: no route to the registry.
    // Not a broken build, and this must not become the check that goes red
    // when a CDN blinks — but it is said out loud, so a run that stopped
    // testing the thing it is named after cannot pass for one that did.
    console.log(
      `install-agent e2e passed the refusal cases; the report itself was not checked because the release archive could not be reached: ${installed.stderr.trim()}`,
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
