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
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
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

/** Whether Nessa pins any Opencode build for this *platform*.
 *
 * Read from the platform, deliberately, and never from what the command said.
 * Windows is the one platform `agent-releases.json` holds nothing for at all,
 * and "nessa installs nothing on Windows" is itself a contract: a Windows pin
 * arriving without the rest of this working should not be silent, so the
 * refusal there is asserted rather than skipped past.
 *
 * Being pinned for is not the same as being served, and this answers only the
 * first. Every x86-64 pin needs AVX2, so a pinned platform still has machines
 * none of its builds runs on — `unserved` below is that half of the question,
 * and it is the half `process.platform` cannot answer. */
const PINNED = process.platform !== "win32"

/** Whether a refusal is "this platform is pinned, and no build fits this
 * machine".
 *
 * `install-agent` has two refusals that both open "nessa has no tested
 * opencode release". One names a platform with no pins behind it at all, which
 * is `PINNED` above. This is the other, and it is the one that depends on more
 * than the platform: builds exist here, and what they ask for — a C library,
 * AVX2 — is not what this machine provides.
 *
 * Matched on the clause only the second sentence carries, because reading one
 * as the other is the difference between "a Windows pin appeared" and "this
 * processor is out of reach". */
function unserved(stderr) {
  return stderr.includes("no tested opencode release this machine can run")
}

/** Whether this processor is one no pinned x86-64 build runs on, asked of the
 * machine rather than of the command.
 *
 * `9ca50a00` dropped the three `-baseline` pins, so every x86-64 build Nessa
 * pins now needs AVX2 and a pre-AVX2 x86-64 machine is served by none of them.
 * That machine is correct and so is the refusal it gets; what was wrong was
 * reading it as a broken install, which is what happens when the platform
 * alone is taken to decide coverage.
 *
 * Read here rather than believed from the refusal, because a refusal taken on
 * its own word is how `host_has_avx2` answering wrongly — on a machine that
 * does have AVX2, whose install should have happened — becomes a green step
 * that verified nothing, with the digest never compared against the registry.
 * The refusal is accepted only where this agrees with it.
 *
 * Answers `false` wherever it cannot tell: an architecture where AVX2 is not a
 * thing to lack, a platform with neither `/proc/cpuinfo` nor `sysctl`, a probe
 * that fails. Each of those leaves a refusal uncorroborated and the run red,
 * which is the direction an unreadable machine should fail in. The
 * architecture read is Node's own, so a Node and a `nessa` built for different
 * architectures — an x86-64 binary under Rosetta beside an arm64 Node — is a
 * red run too, rather than a skip taken on a machine nothing here measured. */
function withoutAvx2() {
  if (process.arch !== "x64" && process.arch !== "ia32") return false
  try {
    if (process.platform === "linux") {
      return !/^flags\s*:.*\bavx2\b/m.test(readFileSync("/proc/cpuinfo", "utf8"))
    }
    if (process.platform === "darwin") {
      const probe = spawnSync("/usr/sbin/sysctl", ["-n", "machdep.cpu.leaf7_features"], {
        encoding: "utf8",
      })
      return probe.status === 0 && !/\bAVX2\b/.test(probe.stdout)
    }
  } catch {
    // An unreadable probe is not evidence of anything, and least of all of the
    // one fact that would let this file stop checking.
    return false
  }
  return false
}

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

/** What each refusal `install-agent` can report means to the two readings
 * above.
 *
 * Held here rather than left to be read, because the fault this guards is one
 * of those conditions quietly growing wider than the single thing it is for,
 * and the run where that matters is the run where it stops checking anything.
 * Each row is a sentence as the command emits it — the `SourceFailure` and
 * `InstallFailure` ones with the retry advice `explain` appends, since the
 * advice is the part that is wider than the fault — followed by what
 * `unreachable` and then `unserved` must say about it.
 *
 * At most one of the two is ever true of a sentence, and the rows where both
 * are false are the failures this file exists to go red on.
 */
const REFUSALS = [
  [
    "could not reach the release archive: connection reset; nothing was installed, try again",
    true,
    false,
  ],
  [
    "could not store the release archive: No space left on device; nothing was installed, try again",
    false,
    false,
  ],
  [
    "the release archive was refused with status 404; the pinned release may have been withdrawn (404)",
    false,
    false,
  ],
  [
    "the archive is not the pinned one; nothing was installed and this is not worth retrying",
    false,
    false,
  ],
  // The machine refusal itself, which is the only sentence that may divert the
  // run into the unserved branch — and which must never be read as a registry
  // this machine could not reach, because those are opposite facts about
  // whether anything was worth downloading.
  [
    "nessa has no tested opencode release this machine can run: it is linux-x86_64 with gnu and no avx2, and the opencode builds for linux-x86_64 need gnu and avx2, or musl and avx2",
    false,
    true,
  ],
  // `explain`'s own unsupported-platform sentence, which says the same thing
  // in the use case's words. The command cannot emit it today — choosing a
  // release refuses first, with the sentence above — and if that ever changes
  // this file should go red and be read rather than quietly take a branch
  // written for a message it no longer gets.
  [
    "no tested release for linux-x86_64 with gnu and no avx2; nothing was installed, and nothing will be until nessa ships a build this machine can run",
    false,
    false,
  ],
]

for (const [message, registry, machine] of REFUSALS) {
  assert.equal(
    unreachable(message),
    registry,
    `${registry ? "must" : "must not"} be treated as an unreachable registry: ${message}`,
  )
  assert.equal(
    unserved(message),
    machine,
    `${machine ? "must" : "must not"} be treated as a machine nothing is pinned for: ${message}`,
  )
}

try {
  // An agent Nessa pins nothing for. The refusal names it, and the one thing
  // that must not happen is a reader being handed a line that is not a report.
  //
  // Claude was this example until it stopped being true: all three agents Nessa
  // drives are pinned now, so asking for Claude here gets the *other* refusal
  // on a machine with no build for it, and on a macOS arm64 runner it gets an
  // 87 MB download this script has no reason to fetch. An agent nessa does not
  // drive is the only thing that still asks the question this case is asking.
  const unknown = install("gemini")
  assert.equal(unknown.status, 25, `expected the agent exit code: ${unknown.stderr}`)
  assert.equal(unknown.stdout, "", `a refusal wrote to stdout: ${unknown.stdout}`)
  assert.match(unknown.stderr, /gemini is not an agent nessa installs/)

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
  } else if (installed.status !== 0 && unserved(installed.stderr)) {
    // A platform Nessa pins builds for, and a machine none of them runs on:
    // at 1.18.31 that is an x86-64 processor without AVX2, which every x86-64
    // pin needs. The refusal is the right answer here, so it is held to being
    // a proper one — the agent exit code, stdout untouched — rather than
    // skipped past, and it is accepted at all only because the processor was
    // asked separately and said the same thing. A refusal this machine
    // contradicts is a regression in `host_has_avx2`, `host_libc` or
    // `satisfies`, and stays a failure.
    assert.ok(
      withoutAvx2(),
      `install-agent refused a machine nothing here found a reason to refuse: ${installed.stderr.trim()}`,
    )
    assert.equal(
      installed.status,
      25,
      `expected the agent exit code: ${installed.stderr}`,
    )
    assert.equal(installed.stdout, "", `a refusal wrote to stdout: ${installed.stdout}`)
    console.log(
      `install-agent e2e passed the refusal cases, including that nessa pins no opencode build this processor can run; there was nothing to install, so the report itself was not checked`,
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
