/** Opt-in acceptance orchestration; no provider, Cargo, or host process probes. */
import { spawn } from "node:child_process"
import { createHash, randomUUID } from "node:crypto"
import { readFile, mkdir, writeFile, realpath, stat } from "node:fs/promises"
import { dirname, isAbsolute, resolve } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
export const scenarios = [
  {
    test: "infrastructure::process::tests::private_directory_is_removed_only_after_process_cleanup_is_confirmed",
    directory: true,
  },
  {
    test: "infrastructure::acp::tests::contracts::shutdown::force_closes_a_term_resistant_parent_and_reaps_its_child",
    directory: false,
  },
].flatMap((scenario) => [false, true].map((init) => ({ ...scenario, init })))

// Libtest panic evidence is correlated to the selected test, not arbitrary output.
function selectedCleanupPanic(output, test) {
  const lines = output.split(/\r?\n/)
  const header = `thread '${test}'`
  return lines.some(
    (line, index) =>
      line.startsWith(header) &&
      /^ (?:\(\d+\) )?panicked at .+:\d+:\d+:$/.test(line.slice(header.length)) &&
      lines[index + 1] ===
        "called `Result::unwrap()` on an `Err` value: CleanupUncertain",
  )
}

export async function publishAcceptance(entry, signal, recordEvidence) {
  const interrupted = async () => {
    const failure = "Harness interrupted before acceptance publication completed"
    await recordEvidence({ ...entry, accepted: false, failure })
    throw new Error(failure)
  }
  // Removal remains independent; this owner settles its subsequent acceptance.
  if (signal.aborted) return interrupted()
  try {
    await recordEvidence({ ...entry, accepted: true })
  } catch (error) {
    await recordEvidence({ ...entry, accepted: false, failure: error.message })
    throw error
  }
  if (signal.aborted) return interrupted()
}

export function validate(report, scenario) {
  const fail = (reason) => {
    throw new Error(`Harness failure: ${reason}`)
  }
  const object = (value) =>
    value !== null && typeof value === "object" && !Array.isArray(value)
  const positivePid = (value) => Number.isSafeInteger(value) && value > 0
  const processEvidence = (value) =>
    object(value) &&
    positivePid(value.pid) &&
    Number.isSafeInteger(value.ppid) &&
    value.ppid >= 0 &&
    positivePid(value.pgid) &&
    typeof value.state === "string" &&
    value.state.length === 1
  if (!object(report)) fail("missing supervisor report")
  if (
    !positivePid(report.supervisor_pid) ||
    !Number.isSafeInteger(report.supervisor_ppid) ||
    report.supervisor_ppid < 0 ||
    !positivePid(report.test_pid)
  )
    fail("missing process identity")
  if (
    typeof report.timed_out !== "boolean" ||
    typeof report.output_truncated !== "boolean"
  )
    fail("missing explicit timeout or truncation evidence")
  if (
    typeof report.test !== "string" ||
    typeof report.output !== "string" ||
    typeof report.pid1_executable !== "string" ||
    !report.pid1_executable.startsWith("/")
  )
    fail("missing test output or PID 1 executable")
  if (
    !Array.isArray(report.initial_processes) ||
    !report.initial_processes.every(processEvidence) ||
    !Array.isArray(report.new_orphans) ||
    !report.new_orphans.every(processEvidence)
  )
    fail("missing process or zombie identity evidence")
  if (
    !Array.isArray(report.retained_directories) ||
    !report.retained_directories.every(
      (path) => typeof path === "string" && /^\/tmp\/nessa-agent-[^/\0]+$/.test(path),
    )
  )
    fail("invalid retained directory evidence")
  if (
    !object(report.packages) ||
    !["python3-minimal", "libgcc-s1", "ca-certificates"].every(
      (name) =>
        Object.hasOwn(report.packages, name) &&
        typeof report.packages[name] === "string" &&
        report.packages[name].trim().length > 0,
    )
  )
    fail("missing prerequisite package versions")
  const initialPids = new Set(report.initial_processes.map((p) => p.pid))
  const initialSupervisor = report.initial_processes.find(
    (p) => p.pid === report.supervisor_pid,
  )
  const initialPid1 = report.initial_processes.find((p) => p.pid === 1)
  if (
    initialPids.size !== report.initial_processes.length ||
    !initialSupervisor ||
    !initialPid1 ||
    initialPid1.ppid !== 0 ||
    initialSupervisor.ppid !== report.supervisor_ppid ||
    initialPids.has(report.test_pid)
  )
    fail("contradictory supervisor or test identity")
  if (
    new Set(report.new_orphans.map((p) => p.pid)).size !== report.new_orphans.length ||
    report.new_orphans.some(
      (p) => initialPids.has(p.pid) || p.pid === report.test_pid || p.ppid !== 1,
    )
  )
    fail("orphan zombie evidence contradicts process identity")
  if (!Number.isInteger(report.test_exit)) fail("missing integer test exit")
  if (report.test !== scenario.test || report.timed_out || report.output_truncated)
    fail("wrong test, timeout or truncated output")
  if (!report.output.includes(`test ${scenario.test} ...`))
    fail("selected test did not run")
  const summary =
    /test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;/.exec(
      report.output,
    )
  if (
    !/\brunning 1 test\b/.test(report.output) ||
    !summary ||
    Number(summary[2]) + Number(summary[3]) !== 1 ||
    Number(summary[4]) !== 0
  )
    fail("exactly one test must run")
  const zombies = report.new_orphans.filter((p) => p.ppid === 1 && p.state === "Z")
  if (scenario.init) {
    if (report.test_exit !== 0 || summary[1] !== "ok" || Number(summary[2]) !== 1)
      fail("init test failed")
    if (zombies.length || report.retained_directories.length)
      fail("init retained zombies or directories")
  } else {
    if (
      report.test_exit === 0 ||
      summary[1] !== "FAILED" ||
      Number(summary[3]) !== 1 ||
      !selectedCleanupPanic(report.output, scenario.test)
    )
      fail("negative case did not fail with CleanupUncertain")
    if (!zombies.length) fail("negative case has no new adopted zombie")
    if (scenario.directory && !report.retained_directories.length)
      fail("negative directory case released its directory")
  }
  if (
    scenario.init
      ? report.supervisor_pid <= 1 || report.supervisor_ppid !== 1
      : report.supervisor_pid !== 1
  )
    fail("unexpected PID namespace supervisor")
  return report
}

export class DockerFailure extends Error {
  constructor(message, stdout, stderr) {
    super(message)
    this.stdout = stdout
    this.stderr = stderr
  }
}

export function docker(
  args,
  { signal, timeout = 30000, launch = spawn, environment = process.env } = {},
) {
  const env = { ...environment }
  for (const key of [
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_TLS",
    "DOCKER_TLS_VERIFY",
    "DOCKER_CERT_PATH",
  ])
    delete env[key]
  return new Promise((resolvePromise, reject) => {
    const child = launch("docker", ["--host=unix:///var/run/docker.sock", ...args], {
      env,
      signal,
      timeout,
      killSignal: "SIGKILL",
    })
    let stdout = "",
      stderr = ""
    const limit = 128 * 1024
    let overflow = false
    const append = (current, data) => {
      const next = current + data
      if (Buffer.byteLength(next) > limit) {
        overflow = true
        child.kill("SIGKILL")
        return Buffer.from(next).subarray(0, limit).toString()
      }
      return next
    }
    child.stdout.on("data", (data) => {
      stdout = append(stdout, data)
    })
    child.stderr.on("data", (data) => {
      stderr = append(stderr, data)
    })
    child.on("error", (error) => reject(new DockerFailure(error.message, stdout, stderr)))
    child.on("close", (code) => {
      if (code === 0 && !overflow) resolvePromise(stdout)
      else
        reject(
          new DockerFailure(
            `docker ${args[0]} ${overflow ? "output exceeded limit" : `exited ${code}`}`,
            stdout,
            stderr,
          ),
        )
    })
  })
}

export async function runScenario(options, scenario, run = docker) {
  const name = `nessa-cleanup-${randomUUID()}`
  const evidence = {
    container: name,
    init: scenario.init,
    test: scenario.test,
    accepted: false,
  }
  const record = async () => {
    await options.recordEvidence?.({ ...evidence })
  }
  try {
    await run(
      [
        "create",
        "--name",
        name,
        ...(scenario.init ? ["--init"] : []),
        "--network=none",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev",
        "--mount",
        `type=bind,src=${options.binary},dst=/probe/nessa-sdk-tests,readonly`,
        "--mount",
        `type=bind,src=${here}/container-init.py,dst=/probe/container-init.py,readonly`,
        "--mount",
        `type=bind,src=${options.fixture},dst=${options.manifest}/tests/infrastructure/acp/contracts/fixtures,readonly`,
        options.image,
        scenario.test,
      ],
      { signal: options.signal },
    )
    const output = await run(["start", "--attach", name], { signal: options.signal })
    evidence.output = output
    // Persist diagnostics before validation can reject and before removal.
    await record()
    evidence.report = JSON.parse(output)
    await record()
    return validate(evidence.report, scenario)
  } catch (error) {
    evidence.failure = error.message
    if (error instanceof DockerFailure) {
      evidence.output = error.stdout
      evidence.stderr = error.stderr
    }
    await record()
    throw error
  } finally {
    // Removal must not inherit an already-aborted signal.
    try {
      await run(["rm", "--force", name], { timeout: 10000 })
    } catch (error) {
      evidence.removalFailure = error.message
      await record()
      throw error
    }
  }
}

export async function checkContainers(args, run = docker, persist = writeFile) {
  const [binaryInput, manifest, image, evidence] = args
  if (
    !binaryInput ||
    !manifest ||
    !image ||
    !evidence ||
    args.length !== 4 ||
    !isAbsolute(manifest)
  ) {
    throw new Error(
      "Usage: node check-container.mjs <explicit-library-test-binary> <absolute-compiled-manifest-dir> <image> <evidence-dir>",
    )
  }
  const binary = await realpath(binaryInput)
  if (!(await stat(binary)).isFile()) throw new Error("Binary must be a file")
  const fixture = resolve(manifest, "tests/infrastructure/acp/contracts/fixtures")
  await stat(resolve(fixture, "claude_acp_test_handler.py"))
  await mkdir(evidence, { recursive: true })
  const controller = new AbortController()
  const interrupt = () => controller.abort()
  process.on("SIGINT", interrupt)
  process.on("SIGTERM", interrupt)
  const reports = new Map()
  try {
    const imageInfo = JSON.parse(
      await run(["image", "inspect", image], { signal: controller.signal }),
    )[0]
    const binarySha256 = createHash("sha256")
      .update(await readFile(binary))
      .digest("hex")
    const recordEvidence = async (entry) => {
      reports.set(entry.container, entry)
      await persist(
        resolve(evidence, "acceptance.json"),
        JSON.stringify(
          {
            image: imageInfo.Id,
            imageReference: image,
            binary,
            binarySha256,
            manifest,
            reports: [...reports.values()],
          },
          null,
          2,
        ) + "\n",
      )
    }
    for (const scenario of scenarios) {
      let captured
      await runScenario(
        {
          binary,
          manifest,
          fixture,
          image: imageInfo.Id,
          signal: controller.signal,
          recordEvidence: async (entry) => {
            captured = entry
            await recordEvidence(entry)
          },
        },
        scenario,
        run,
      )
      await publishAcceptance(captured, controller.signal, recordEvidence)
      console.log(`${scenario.init ? "init" : "non-reaping"}: ${scenario.test}: accepted`)
    }
  } finally {
    process.removeListener("SIGINT", interrupt)
    process.removeListener("SIGTERM", interrupt)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  checkContainers(process.argv.slice(2)).catch((error) => {
    console.error(error)
    process.exitCode = 1
  })
}
