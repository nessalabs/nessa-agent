import assert from "node:assert/strict"
import { readdirSync, readFileSync } from "node:fs"
import test from "node:test"
import { RELEASE_TARGETS, releaseBundles } from "./release-assets.mjs"

test("local-auth integration harness is locked to its declared tools", () => {
  const manifest = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package.json", "utf8"),
  )
  const lock = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package-lock.json", "utf8"),
  )
  assert.deepEqual(lock.packages[""].dependencies, manifest.dependencies)
  assert.deepEqual(manifest.dependencies, {
    "@nessa/client": "file:./packages/nessa-client",
    tsx: "4.23.13",
    ws: "8.18.3",
  })
  assert.deepEqual(lock.packages["node_modules/@nessa/client"], {
    resolved: "packages/nessa-client",
    link: true,
  })
  for (const dependency of ["tsx", "ws"]) {
    const installed = lock.packages[`node_modules/${dependency}`]
    assert.equal(installed.version, manifest.dependencies[dependency])
    assert.match(installed.integrity, /^sha512-/)
  }
})

test("local and CI aggregate the same named frontend and native checks", () => {
  const root = JSON.parse(readFileSync("package.json", "utf8"))
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.match(root.scripts.check, /pnpm frontend:check/)
  for (const [aggregate, script, command] of [
    ["check", "frontend:check", "pnpm frontend:check"],
    ["sdk:check", "sdk:docs:check", "node scripts/check-sdk-docs.mjs"],
    ["check", "mcp:check", "node scripts/check-mcp.mjs"],
    ["check", "desktop:check", "node scripts/check-desktop.mjs"],
  ]) {
    assert.ok(root.scripts[aggregate].includes(`pnpm ${script}`))
    assert.ok(workflow.includes(`run: ${command}`), `CI is missing ${script}`)
  }
  assert.match(
    workflow,
    /cargo fmt -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk -- --check/,
  )
  assert.equal(
    root.scripts.architecture,
    "node --test scripts/architecture/*.test.mjs && node scripts/check-architecture.mjs",
  )
  assert.match(workflow, /node --test scripts\/architecture\/\*\.test\.mjs/)
  assert.match(workflow, /node scripts\/check-architecture\.mjs/)
  // A deadlocked test fails the step in minutes, not at the job's six-hour
  // limit (#366). The package list is one build; the runner overlaps binaries.
  assert.match(
    workflow,
    /run: node scripts\/cargo-test-parallel\.mjs --concurrency 2 -- -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk\r?\n\s+timeout-minutes: \d+\r?\n/,
  )
  assert.doesNotMatch(
    workflow,
    /run: cargo test -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk\b/,
  )
  // The coverage gate runs the same SDK tests again, instrumented, in its own job.
  assert.match(
    workflow,
    /run: bash scripts\/check-sdk-domain-coverage\.sh\r?\n\s+timeout-minutes: \d+\r?\n/,
  )
  assert.match(
    workflow,
    /cargo clippy -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-protocol -p nessa-client-core -p nessa-sdk --all-targets -- -D warnings/,
  )
  // The database opener's privacy checks answer differently on each OS, so
  // they run in the matrix, as `pnpm check` runs them locally.
  assert.match(root.scripts.check, /pnpm database:check/)
  assert.match(workflow, /run: cargo test -p nessa-local-database/)
  assert.match(
    workflow,
    /run: cargo clippy -p nessa-local-database --all-targets -- -D warnings/,
  )
  assert.match(workflow, /npm ci --ignore-scripts/)
})

test("Linux coverage is its own job, and assembly stays with the native smoke", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  // Hosted Windows checks this file out with CRLF. A search that requires a
  // bare LF after the job key misses every header there, and check-desktop
  // fails the matrix leg after the Rust tests have already passed.
  const asWindowsCheckout = workflow.replace(/\r\n/g, "\n").replace(/\n/g, "\r\n")
  for (const checkout of [workflow, asWindowsCheckout]) {
    const jobAt = (name) => checkout.search(new RegExp(`\\n  ${name}:\\r?\\n`))
    const localAuthStart = jobAt("local-auth")
    const coverageStart = jobAt("sdk-domain-coverage")
    const checksStart = jobAt("required-checks")
    assert.ok(localAuthStart !== -1, "local-auth job")
    assert.ok(coverageStart > localAuthStart, "sdk-domain-coverage follows local-auth")
    assert.ok(checksStart > coverageStart, "required-checks follows sdk-domain-coverage")
    const localAuth = checkout.slice(localAuthStart, coverageStart)
    const coverage = checkout.slice(coverageStart, checksStart)
    assertCoverageJobSplit(localAuth, coverage, checkout)
  }
})

function assertCoverageJobSplit(localAuth, coverage, workflow) {
  assert.match(localAuth, /name: Assemble the Linux desktop runtime/)
  assert.match(localAuth, /pnpm desktop:smoke/)
  assert.doesNotMatch(localAuth, /run: bash scripts\/check-sdk-domain-coverage\.sh/)
  assert.match(coverage, /bash scripts\/check-sdk-domain-coverage\.sh/)
  assert.match(coverage, /NESSA_SDK_COVERAGE_TARGET: target\/sdk-domain-coverage/)
  assert.match(coverage, /prefix-key: sdk-domain-coverage/)
  assert.match(coverage, /cache-workspace-crates: "true"/)
  assert.doesNotMatch(coverage, /prepare\.mjs|desktop:smoke/)
  assert.equal(workflow.match(/uses: Swatinem\/rust-cache@v2/g)?.length, 4)
  assert.equal(
    workflow.match(/github\.event\.inputs\.rust-cache != 'cold'/g)?.length,
    4,
    "a cold measurement skips the cache and nothing else",
  )
  assert.match(workflow, /options:\s+- restore\s+- cold\s+default: restore/)
}

test("the existing Linux matrix leg uniquely owns direct runtime assembly", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.equal(workflow.match(/run: node scripts\/desktop\/prepare\.mjs/g)?.length, 1)
  assert.match(
    workflow,
    /name: Assemble the Linux desktop runtime\s+if: runner\.os == 'Linux'\s+run: node scripts\/desktop\/prepare\.mjs/,
  )
  // The release assembles the runtime only inside the bundle build, through
  // the config's beforeBuildCommand, so what it verifies is what it packaged.
  const releaseWorkflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.doesNotMatch(releaseWorkflow, /scripts\/desktop\/prepare\.mjs/)
  assert.match(
    releaseWorkflow,
    /target: x86_64-unknown-linux-gnu\s+updater-target: linux-x86_64\s/,
  )
  assert.match(
    releaseWorkflow,
    /run: pnpm app:build --target \$\{\{ matrix\.target \}\} \$\{\{ matrix\.bundles && format\('--bundles \{0\}', matrix\.bundles\) \|\| '' \}\}/,
  )
})

test("every Linux build installs the one list of host build dependencies", () => {
  const install = "bash scripts/desktop/install-linux-build-deps.sh"
  for (const name of readdirSync(".github/workflows").filter((file) =>
    /\.ya?ml$/.test(file),
  )) {
    // Continuation lines joined, so a package list below `apt-get install \`
    // is on the same line as the command that installs it.
    const workflow = readFileSync(`.github/workflows/${name}`, "utf8").replace(
      /\\\r?\n\s*/g,
      " ",
    )
    // A development package installed by hand is a second list of what the
    // host builds against.
    assert.doesNotMatch(
      workflow,
      /apt-get install[^\n]*-dev\b/,
      `${name} installs its own list`,
    )
    if (/pnpm app:build|check-desktop\.mjs|desktop:smoke/.test(workflow))
      assert.ok(workflow.includes(install), `${name} builds the host without the list`)
  }
})

/**
 * Job keys sit at two spaces. A job timeout sits at four, which is what keeps
 * it distinct from a step timeout further in.
 *
 * GitHub's job id is a letter or `_`, then letters, digits, `_`, or `-`.
 * There is no YAML parser in the dev dependencies, so this is that syntax.
 * A narrower `[a-z][a-z0-9-]*` class folds `sdk_coverage`, `Build`, and
 * `_hidden` into the previous job, and a missing timeout on them passes.
 */
function workflowJobs(workflow) {
  const jobsAt = workflow.search(/\njobs:\r?\n/)
  if (jobsAt === -1) return []
  // `on:` uses the same two-space keys (`push:`, `pull_request:`). Jobs are
  // only what follows `jobs:`.
  const jobs = workflow.slice(jobsAt)
  const headers = [...jobs.matchAll(/\n  ([A-Za-z_][A-Za-z0-9_-]*):\r?\n/g)]
  return headers.map((header, index) => {
    const start = header.index + 1
    const end = index + 1 < headers.length ? headers[index + 1].index + 1 : jobs.length
    return [header[1], jobs.slice(start, end)]
  })
}

function jobTimeoutMinutes(body) {
  return [...body.matchAll(/^    timeout-minutes: (\d+)\r?$/gm)].map((match) =>
    Number(match[1]),
  )
}

/**
 * Worst case of install-linux-build-deps.sh, in minutes. Each `timeout`
 * duration is followed by its kill-after when the command ignores SIGTERM,
 * and the backoff sleeps run between update attempts, not after the last one.
 */
function aptScriptBudgetMinutes(script) {
  const update = script.match(
    /timeout --kill-after=(\d+)s (\d+)m apt-get "\$\{apt_options\[@\]\}" update/,
  )
  const install = script.match(
    /timeout --kill-after=(\d+)s (\d+)m apt-get "\$\{apt_options\[@\]\}" install/,
  )
  assert.ok(update, "update timeout")
  assert.ok(install, "install timeout")
  const killSec = Number(update[1])
  assert.equal(Number(install[1]), killSec)
  const attempts = script
    .match(/for attempt in ([0-9 ]+);/)[1]
    .trim()
    .split(/\s+/)
    .map(Number)
  const backoffFactor = Number(script.match(/sleep \$\(\(attempt \* (\d+)\)\)/)[1])
  const backoffSec = attempts
    .slice(0, -1)
    .reduce((sum, attempt) => sum + attempt * backoffFactor, 0)
  return (
    attempts.length * (Number(update[2]) + killSec / 60) +
    backoffSec / 60 +
    Number(install[2]) +
    killSec / 60
  )
}

test("apt installs and every CI job are bounded", () => {
  // #653. A silent mirror used to hold the install step until GitHub's
  // six-hour job limit, because nothing around apt-get had a bound.
  const script = readFileSync("scripts/desktop/install-linux-build-deps.sh", "utf8")
  assert.match(script, /Acquire::Retries=3/)
  assert.match(script, /Acquire::http::Timeout=30/)
  assert.match(script, /Acquire::https::Timeout=30/)
  assert.doesNotMatch(script, /sudo\s+-E\b/)
  assert.equal(script.match(/apt-get "\$\{apt_options\[@\]\}"/g)?.length, 2)
  // `update` is the command the mirror stalled in, so it is the one that is
  // retried. `install` stays after the loop: one attempt, its own ceiling.
  assert.match(
    script,
    /for attempt in 1 2 3; do\r?\n {2}if sudo DEBIAN_FRONTEND=noninteractive timeout --kill-after=30s 5m apt-get "\$\{apt_options\[@\]\}" update; then\r?\n {4}break\r?\n {2}fi\r?\n {2}if \[ "\$attempt" -eq 3 \]; then\r?\n {4}echo "apt-get update failed on all three attempts" >&2\r?\n {4}exit 1\r?\n {2}fi\r?\n {2}sleep \$\(\(attempt \* 15\)\)\r?\ndone\r?\nsudo DEBIAN_FRONTEND=noninteractive timeout --kill-after=30s 45m apt-get "\$\{apt_options\[@\]\}" install -y \\/,
  )
  // 3×(5m+30s) + 15s + 30s + 45m + 30s. The install step has to cover it.
  const scriptBudget = aptScriptBudgetMinutes(script)
  assert.equal(scriptBudget, 62.75)

  // Minutes, from 21 green local-auth runs on 2026-10-07 and the four release
  // runs that exist. local-auth is 130 because the 65 minute apt step, the
  // 45 minute Rust test step, and at most 11.3 minutes of everything else
  // sum past 120. The release build is 120 because 65 + 25.7 does too.
  const bounds = {
    "local-auth.yml": {
      workflows: 10,
      changes: 10,
      "gateway-contract": 45,
      "desktop-release-profile": 45,
      frontend: 30,
      "local-auth": 130,
      "sdk-domain-coverage": 60,
      "required-checks": 10,
    },
    "release.yml": {
      version: 10,
      "updater-key": 30,
      build: 120,
      release: 15,
    },
  }
  for (const [file, expected] of Object.entries(bounds)) {
    const original = readFileSync(`.github/workflows/${file}`, "utf8")
    // Hosted Windows checks the workflow out with CRLF. A job split or a
    // timeout line that requires a bare LF passes here and fails that leg.
    const asWindowsCheckout = original.replace(/\r\n/g, "\n").replace(/\n/g, "\r\n")
    for (const workflow of [original, asWindowsCheckout]) {
      const jobs = workflowJobs(workflow)
      assert.deepEqual(
        jobs.map(([name]) => name).sort(),
        Object.keys(expected).sort(),
        `${file} jobs`,
      )
      for (const [name, body] of jobs) {
        assert.deepEqual(jobTimeoutMinutes(body), [expected[name]], `${file} ${name}`)
      }
    }
  }

  const localAuth = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const release = readFileSync(".github/workflows/release.yml", "utf8")
  for (const workflow of [localAuth, release]) {
    const asWindowsCheckout = workflow.replace(/\r\n/g, "\n").replace(/\n/g, "\r\n")
    const step =
      workflow === localAuth
        ? /run: bash scripts\/desktop\/install-linux-build-deps\.sh webkit2gtk-driver xvfb\r?\n\s+timeout-minutes: (\d+)\r?\n/
        : /run: bash scripts\/desktop\/install-linux-build-deps\.sh\r?\n\s+timeout-minutes: (\d+)\r?\n/
    for (const checkout of [workflow, asWindowsCheckout]) {
      const found = checkout.match(step)
      assert.ok(found, "install step timeout")
      const stepTimeout = Number(found[1])
      assert.equal(stepTimeout, 65)
      // Script budget, then the step, then the job. The step has to be able
      // to elapse; the job has to still be running when it does.
      const jobTimeout =
        workflow === localAuth
          ? bounds["local-auth.yml"]["local-auth"]
          : bounds["release.yml"].build
      assert.ok(scriptBudget <= stepTimeout, `${scriptBudget} <= ${stepTimeout}`)
      assert.ok(stepTimeout < jobTimeout, `${stepTimeout} < ${jobTimeout}`)
    }
  }
})

test("an underscore or uppercase job with no timeout is not absorbed by the previous job", () => {
  const fixture = [
    "name: Fixture",
    "on:",
    "  push:",
    "  pull_request:",
    "jobs:",
    "  local-auth:",
    "    timeout-minutes: 130",
    "    steps:",
    "      - run: echo ok",
    "  sdk_coverage:",
    "    runs-on: ubuntu-latest",
    "    steps:",
    "      - run: echo missing",
    "  Build:",
    "    runs-on: ubuntu-latest",
    "    steps:",
    "      - run: echo also missing",
    "  _hidden:",
    "    runs-on: ubuntu-latest",
    "    steps:",
    "      - run: echo leading underscore",
    "",
  ].join("\n")
  for (const checkout of [fixture, fixture.replace(/\n/g, "\r\n")]) {
    const jobs = workflowJobs(checkout)
    assert.deepEqual(
      jobs.map(([name]) => name),
      ["local-auth", "sdk_coverage", "Build", "_hidden"],
    )
    const missing = jobs
      .filter(([, body]) => jobTimeoutMinutes(body).length !== 1)
      .map(([name]) => name)
    assert.deepEqual(missing, ["sdk_coverage", "Build", "_hidden"])
  }
})

test("the existing Windows matrix leg uniquely owns the Task Scheduler model proof", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const proof = "./scripts/desktop/check-windows-task-scheduler.ps1"
  // Same job split as the timeout check. A narrower class would swallow a
  // following `sdk_coverage` into this body and the proof would still look unique.
  const localAuth = workflowJobs(workflow).find(([name]) => name === "local-auth")?.[1]
  assert.ok(localAuth, "local-auth job")
  assert.match(localAuth, /matrix:\s+os: \[windows-latest, ubuntu-latest, macos-latest\]/)
  assert.equal(localAuth.split(proof).length - 1, 1)
  assert.match(
    localAuth,
    /name: Prove the Windows Task Scheduler gateway model\s+if: runner\.os == 'Windows'\s+shell: powershell\s+run: \.\/scripts\/desktop\/check-windows-task-scheduler\.ps1 -CallerContext Administrator\r?\n/,
  )
  const invocations = readdirSync(".github/workflows")
    .filter((name) => /\.ya?ml$/.test(name))
    .map((name) => readFileSync(`.github/workflows/${name}`, "utf8"))
    .reduce(
      (count, contents) =>
        count + (contents.match(/check-windows-task-scheduler\.ps1/g)?.length ?? 0),
      0,
    )
  assert.equal(
    invocations,
    1,
    "the native proof must run once in the existing Windows matrix leg",
  )
})

test("the Windows scheduler proof binds identity and cleanup to one exact owned task", () => {
  const script = readFileSync("scripts/desktop/check-windows-task-scheduler.ps1", "utf8")
  assert.match(script, /CreateDirectoryW/)
  assert.match(script, /SECURITY_ATTRIBUTES/)
  assert.match(script, /FILE_FLAG_OPEN_REPARSE_POINT/)
  assert.doesNotMatch(script, /FILE_SHARE_DELETE/)
  assert.match(script, /DirectoryIdentity\(\$runRootHandle\)/)
  assert.match(script, /MarkDirectoryForDeletion\(\$runRootHandle\)/)
  assert.doesNotMatch(script, /Directory\]::CreateDirectory|Directory\.CreateDirectory/)
  assert.match(script, /RegisterTaskDefinition\(\$taskName, \$definition, 2 -bor 16,/)
  assert.match(script, /MultipleInstances = 2/)
  assert.equal(script.match(/\$ownedTask\.Run\(\$null\)/g)?.length, 2)
  assert.match(script, /ActionPid', 'EnginePid'/)
  assert.match(script, /PlannedActionId', 'CurrentAction'/)
  assert.match(script, /OpenProcessForObservation\(\[uint32\]\$instance\.EnginePID\)/)
  assert.match(
    script,
    /OpenProcess\(PROCESS_QUERY_LIMITED_INFORMATION \| SYNCHRONIZE, false, processId\)/,
  )
  assert.match(
    script,
    /\$caller = \[NessaWindowsProofNative\]::ReadCurrentProcessTokenFacts\(\)/,
  )
  assert.match(
    script,
    /\[ValidateSet\('StandardUser', 'Administrator'\)\]\s+\[string\] \$CallerContext = 'StandardUser'/,
  )
  assert.match(
    script,
    /if \(\$CallerContext -eq 'StandardUser' -and \$caller\.Elevated\) \{ throw /,
  )
  assert.match(
    script,
    /if \(\$CallerContext -eq 'Administrator' -and -not \$caller\.Elevated\) \{ throw /,
  )
  assert.doesNotMatch(
    script,
    /LogonType\s*=\s*(1|2|5|6)\b|TASK_LOGON_(PASSWORD|S4U|GROUP)/,
  )
  assert.doesNotMatch(script, /Xml -match/)
  assert.match(script, /Test-TaskXmlCarriesNoCredential -Xml \$observedTask\.Xml/)
  assert.match(script, /^Assert-CredentialFieldProbes\r?$/m)
  assert.doesNotMatch(script, /OpenProcessForObservation[^\r\n]*\$PID/)
  assert.doesNotMatch(script, /\$callerHandle\s*=/)
  assert.match(script, /OpenProcessToken failed for current-process pseudo-handle/)
  const tokenInformationBoundary =
    /private const int ERROR_BAD_LENGTH = 24;\s+private const int ERROR_INSUFFICIENT_BUFFER = 122;[\s\S]*?\[DllImport\("advapi32\.dll", SetLastError = true\)\]\s+private static extern bool GetTokenInformation\(\s*IntPtr token, int informationClass, IntPtr information, int length, out int returnLength\);[\s\S]*?private static byte\[\] TokenInformation\(IntPtr token, int informationClass, string fact\)\s*\{\s*int needed;\s*bool sized = GetTokenInformation\(token, informationClass, IntPtr\.Zero, 0, out needed\);\s*int error = Marshal\.GetLastWin32Error\(\);\s*if \(needed <= 0 \|\| \(!sized && error != ERROR_BAD_LENGTH && error != ERROR_INSUFFICIENT_BUFFER\)\)\s*throw new Win32Exception\(error, "GetTokenInformation size query failed for " \+ fact\);\s*var bytes = new byte\[needed\];\s*var pinned = GCHandle\.Alloc\(bytes, GCHandleType\.Pinned\);\s*try\s*\{\s*if \(!GetTokenInformation\(token, informationClass, pinned\.AddrOfPinnedObject\(\), bytes\.Length, out needed\)\)\s*throw new Win32Exception\(Marshal\.GetLastWin32Error\(\), "GetTokenInformation fill failed for " \+ fact\);\s*return bytes;\s*\}\s*finally \{ pinned\.Free\(\); \}\s*\}/g
  assert.equal(
    script.match(tokenInformationBoundary)?.length,
    1,
    "one native token-information boundary must own sizing, allocation, fill, and release",
  )
  assert.match(script, /ReadTokenFacts\(\$engineProcess\)/)
  assert.match(script, /ProcessHasExited\(\$engineProcess\)/)
  assert.match(script, /ActionCreationTime', 'PidBoundCreationTime'/)
  assert.doesNotMatch(script, /ReadTokenFacts\(\[uint32\]/)
  assert.match(script, /Get-NativeException/)
  assert.match(script, /MethodInvocationException/)
  assert.match(script, /Assert-HResultClassification/)
  assert.match(script, /AceSids = \[string\]::Join/)
  assert.match(script, /ObjectAceCount/)
  assert.match(script, /confirmed-created-contradictory/)
  assert.match(script, /observed-matching-after-\$Acknowledgement-reply/)
  assert.match(script, /Assert-LifecycleStateProbes/)
  assert.match(script, /New-RunAttemptObservation/)
  assert.match(script, /function Invoke-RunAttemptObservation/)
  assert.match(script, /Assert-RunObservationAccepted/)
  assert.match(script, /Test-StabilizedRunSnapshot/)
  assert.equal(
    script.match(/Test-StabilizedRunSnapshot -Accepted/g)?.length,
    1,
    "the single observation owner must enforce the complete stabilized process vector",
  )
  assert.equal(
    script.match(/Test-CompleteAgreement -Evidence/g)?.length,
    1,
    "the single observation owner must enforce the broader accepted vector",
  )
  assert.match(
    script,
    /Invoke-RunAttemptObservation -Observation \$firstRunObservation -Instances \$instances/,
  )
  assert.match(
    script,
    /Invoke-RunAttemptObservation -Observation \$firstRunObservation -Instances \$firstInstances[^\r\n]+-Snapshot \$firstSnapshot -CompleteAcceptance -Final/,
  )
  assert.match(
    script,
    /Invoke-RunAttemptObservation -Observation \$secondRunObservation -Instances \$instances/,
  )
  assert.match(
    script,
    /Invoke-RunAttemptObservation -Observation \$secondRunObservation -Instances \$secondInstances[^\r\n]+-Snapshot \$secondSnapshot -CompleteAcceptance -Final/,
  )
  assert.match(script, /first run 2 to 1/)
  assert.match(script, /second run contradiction to match/)
  assert.match(script, /matching poll followed by a contradictory final snapshot/)
  assert.match(
    script,
    /matching final observation omitted its complete process vector without rejection/,
  )
  assert.match(
    script,
    /complete observation owner accepted mutation of \$\(\$entry\.Key\)/,
  )
  assert.match(script, /retained action process exited before proof completion/)
  assert.match(script, /Invoke-StopSettlement/)
  assert.match(script, /\$runEffectsSettled = \$stopSettlement\.Settled/)
  assert.match(script, /function Invoke-DependencyOrderedCleanup/)
  assert.match(script, /\$taskSettledBeforeDelete = Test-CreateEffectSettled/)
  assert.match(script, /\$folderSettledBeforeDelete = Test-CreateEffectSettled/)
  assert.match(script, /preserved task create effect/)
  assert.match(script, /uncertain folder create effect/)
  assert.match(script, /lost stop reply with confirmed settlement/)
  assert.match(script, /Invoke-DeleteSettlement/)
  assert.match(script, /DeleteOutcome = \$deleteOutcome/)
  assert.match(script, /ObservationDiagnostic = \$observationDiagnostic/)
  assert.match(script, /\$ownedTask\.Stop\(0\)/)
  assert.match(script, /exact owned task instance collection to become empty/)
  assert.doesNotMatch(script, /Stop-Process|\.Kill\(|TerminateProcess/)
  assert.match(script, /primary failure:/)
  assert.match(script, /cleanup failure:/)
})

test("the disposable user manager proves the same session bus and cleans its exact runtime", () => {
  const script = readFileSync("scripts/desktop/check-systemd-user-service.sh", "utf8")
  assert.match(script, /systemctl --user show-environment/)
  assert.match(script, /systemd-run --user --pipe --wait --collect --quiet/)
  assert.match(script, /--unit="\$transient_unit"/)
  assert.match(script, /-p Delegate=yes -p Type=exec -d/)
  assert.match(script, /export XDG_RUNTIME_DIR="\$RUN_DIR"/)
  assert.match(script, /export XDG_CONFIG_HOME="\$RUN_DIR\/config"/)
  assert.match(script, /export XDG_DATA_HOME="\$RUN_DIR\/data"/)
  assert.match(script, /export XDG_STATE_HOME="\$RUN_DIR\/state"/)
  assert.match(script, /export XDG_CACHE_HOME="\$RUN_DIR\/cache"/)
  assert.match(
    script,
    /export SYSTEMD_ENVIRONMENT_GENERATOR_PATH="\$RUN_DIR\/empty-environment-generators"/,
  )
  assert.match(script, /export SYSTEMD_GENERATOR_PATH="\$RUN_DIR\/empty-generators"/)
  assert.match(script, /export SYSTEMD_UNIT_PATH="\$RUN_DIR\/systemd\/user:/)
  assert.match(script, /systemd --user --unit=basic\.target/)
  assert.match(script, /unset DBUS_SESSION_BUS_ADDRESS/)
  assert.match(script, /systemctl --user start dbus\.socket/)
  assert.match(
    script,
    /export DBUS_SESSION_BUS_ADDRESS="unix:path=\$XDG_RUNTIME_DIR\/bus"/,
  )
  assert.doesNotMatch(script, /dbus-run-session/)
  assert.match(script, /disposable systemd user manager ready at/)
  assert.match(script, /disposable systemd gateway lifecycle completed/)
  assert.match(script, /SYSTEMD_LOG_LEVEL=debug SYSTEMD_LOG_TARGET=console/)
  assert.match(script, /kill -0 "\$manager_pid"/)
  assert.match(script, /manager exited before readiness with status \$manager_status/)
  assert.match(script, /-S "\$XDG_RUNTIME_DIR\/systemd\/private"/)
  assert.match(script, /busctl --address="\$DBUS_SESSION_BUS_ADDRESS"/)
  assert.match(script, /status org\.freedesktop\.systemd1/)
  assert.match(script, /2>"\$bus_error"/)
  assert.doesNotMatch(script, /busctl --user/)
  assert.doesNotMatch(script, /loginctl|enable-linger/)
  assert.match(
    script,
    /inaccessible_directory="\$runtime_directory\/systemd\/inaccessible\/dir"/,
  )
  assert.match(script, /chmod 700 "\$inaccessible_directory"/)
  assert.doesNotMatch(script, /chmod -R|find .* -delete/)
  assert.match(script, /primary_status=\$\?/)
  assert.match(script, /if \[ "\$primary_status" -eq 0 \]/)
  assert.equal(script.match(/print_manager_diagnostics/g)?.length, 5)
})

test("the frontend job owns top-level script and verification-script tests, and their just dependency", () => {
  const root = JSON.parse(readFileSync("package.json", "utf8"))
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const gatewayStart = workflow.indexOf("  gateway-contract:")
  const releaseStart = workflow.indexOf("  desktop-release-profile:")
  const frontendStart = workflow.indexOf("  frontend:")
  const localAuthStart = workflow.indexOf("  local-auth:")
  for (const boundary of [gatewayStart, releaseStart, frontendStart, localAuthStart])
    assert.notEqual(boundary, -1, "expected CI job boundary is missing")
  const gateway = workflow.slice(gatewayStart, releaseStart)
  const frontend = workflow.slice(frontendStart, localAuthStart)

  assert.equal(
    root.scripts["scripts:test"],
    "node --test scripts/*.test.mjs scripts/mcp-test-server/*.test.mjs scripts/subagent-contracts/*.test.mjs scripts/process-cleanup/*.test.mjs",
  )
  assert.ok(root.scripts["frontend:check"].includes("pnpm scripts:test"))
  // Every verification-script test under verification/desktop/scripts/lib,
  // #384's design table among them.
  assert.equal(
    root.scripts["verify:desktop:test"],
    "node --test verification/desktop/scripts/lib/*.test.mjs",
  )
  assert.ok(root.scripts["frontend:check"].includes("pnpm verify:desktop:test"))
  assert.doesNotMatch(workflow, /run: node --test scripts\/\*\.test\.mjs/)
  assert.doesNotMatch(gateway, /node --test scripts\/\*\.test\.mjs/)
  assert.doesNotMatch(gateway, /install-action@just/)
  assert.equal(workflow.match(/install-action@just/g)?.length, 1)
  assert.match(
    frontend,
    /install-action@just[\s\S]*run: pnpm frontend:check/,
    "the suite's just prerequisite must be installed before its owning aggregate",
  )
})

test("nothing in a release is built or published before the key pairing gate", () => {
  // The one release failure with no remedy: ship a build whose trusted public
  // key is not the other half of the signing key, and every update it will ever
  // see is rejected forever. The gate that settles it must stay ahead of the
  // builds, and must stay in its demanding mode — its default when no key is
  // reachable is a *skip*, which in a release would read as a pass.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(
    workflow,
    /^\s*- run: cargo test -p nessa-app --test updater_key_pairing --no-default-features\s*$/m,
  )
  assert.match(workflow, /NESSA_REQUIRE_UPDATER_KEY_PAIRING: "1"/)
  assert.match(workflow, /needs: \[version, updater-key\]/)
  assert.match(workflow, /needs: \[version, build\]/)
})

test("a release builds every release target and only the bundles it publishes", () => {
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  // One matrix row per target the manifest requires: a target built nowhere is
  // a manifest that can never be written, and a row for a target the manifest
  // does not know is a build whose output is thrown away. Separate runners,
  // because neither prepare script cross-compiles and there is no universal
  // build.
  const rows = [
    ...workflow.matchAll(
      /target: (\S+)\s+updater-target: \S+(?:\s+bundles: ([^\s#]+))?/g,
    ),
  ]
  assert.deepEqual(
    rows.map(([, target]) => target),
    RELEASE_TARGETS,
  )
  // The shipped config says "all" and stays that way for local macOS builds; a
  // release row narrows it to what an update installs and a person downloads.
  // A Linux row names none: the build command makes exactly the release's
  // bundles there and refuses to be told otherwise.
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
  assert.equal(config.bundle.targets, "all")
  for (const [, target, bundles] of rows)
    assert.equal(
      bundles,
      target.endsWith("-linux-gnu") ? undefined : releaseBundles(target),
      `${target} builds bundles its release does not publish`,
    )
})

test("a release is staged as a draft, never published by the workflow", () => {
  // `releases/latest/download/latest.json` resolves only to a published,
  // non-prerelease release, so a draft offers nothing to anyone until a person
  // publishes it. Publishing is the deliberate act that offers the update, and
  // it stays a person's.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /gh release create "\$TAG" --draft/)
  assert.doesNotMatch(workflow, /--draft=false|gh release edit .*--draft/)
})

/**
 * The notes say where to look, rather than asserting a fact about signing.
 *
 * They used to state flatly that the artifacts were not notarized, which was
 * true when nothing had ever been built and false the moment the secrets were
 * set — and nothing would have corrected it. What is actually true of every
 * build is that the build says which of the two it did.
 */
test("the draft's notes point at the build's own account of what it signed with", () => {
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /Choose what this build signs with/)
  assert.doesNotMatch(
    workflow,
    /\*\*These artifacts are not notarized\.\*\*/,
    "the notes state something about signing that no build checked",
  )
})

test("a release builds the commit its tag names, not a branch of the same name", () => {
  // `actions/checkout` takes an unqualified ref and looks for a branch before a
  // tag, so a branch called `v0.1.0` would be what got built. Comparing declared
  // versions cannot catch it: both commits can say the same version. The tag is
  // resolved to a commit once, and every job builds that commit.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /git\/ref\/tags\//)
  assert.doesNotMatch(
    workflow,
    /ref: \$\{\{ (needs\.version\.outputs\.tag|steps\.resolve\.outputs\.tag) \}\}/,
    "a job is still checking out a name rather than the resolved commit",
  )
  for (const match of workflow.matchAll(/ref: \$\{\{ ([^}]+) \}\}/g))
    assert.match(match[1], /\.sha\b/, `checkout of ${match[1].trim()} is not a commit`)
})

test("publication requires the tag to exist and states the channel", () => {
  // `gh release create` creates a missing tag from the default branch, which
  // would publish code nobody tagged; `--verify-tag` refuses instead. And
  // GitHub keeps pre-release apart from the tag's spelling, so an rc published
  // without the flag is an ordinary release — the one the updater serves.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /gh release create "\$TAG" --draft --verify-tag/)
  assert.match(workflow, /\$\{PRERELEASE:\+--prerelease\}/)
  // A draft from a run before the channel was set is corrected rather than left.
  assert.match(workflow, /gh release edit "\$TAG" --prerelease=/)
})
