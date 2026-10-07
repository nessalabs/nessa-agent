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

export function validate(report, scenario) {
  const fail = (reason) => {
    throw new Error(`Harness failure: ${reason}`)
  }
  if (report.test !== scenario.test || report.timed_out) fail("wrong test or timeout")
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
      !/\bCleanupUncertain\b/.test(report.output)
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

export function docker(args, { signal, timeout = 30000 } = {}) {
  const env = { ...process.env }
  for (const key of [
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_TLS",
    "DOCKER_TLS_VERIFY",
    "DOCKER_CERT_PATH",
  ])
    delete env[key]
  return new Promise((resolvePromise, reject) => {
    const child = spawn("docker", ["--host=unix:///var/run/docker.sock", ...args], {
      env,
      signal,
      timeout,
      killSignal: "SIGKILL",
    })
    let stdout = "",
      stderr = ""
    child.stdout.on("data", (data) => {
      stdout += data
    })
    child.stderr.on("data", (data) => {
      stderr += data
    })
    child.on("error", reject)
    child.on("close", (code) =>
      code === 0
        ? resolvePromise(stdout)
        : reject(new Error(`docker ${args[0]} exited ${code}: ${stderr}`)),
    )
  })
}

export async function runScenario(options, scenario, run = docker) {
  const name = `nessa-cleanup-${randomUUID()}`
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
    return validate(JSON.parse(output), scenario)
  } finally {
    // Removal must not inherit an already-aborted signal.
    await run(["rm", "--force", name], { timeout: 10000 })
  }
}

async function main() {
  const [binaryInput, manifest, image, evidence] = process.argv.slice(2)
  if (
    !binaryInput ||
    !manifest ||
    !image ||
    !evidence ||
    process.argv.length !== 6 ||
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
  process.once("SIGINT", interrupt)
  process.once("SIGTERM", interrupt)
  const reports = []
  try {
    const imageInfo = JSON.parse(await docker(["image", "inspect", image]))[0]
    const binarySha256 = createHash("sha256")
      .update(await readFile(binary))
      .digest("hex")
    for (const scenario of scenarios) {
      const report = await runScenario(
        { binary, manifest, fixture, image, signal: controller.signal },
        scenario,
      )
      reports.push({ init: scenario.init, ...report })
      await writeFile(
        resolve(evidence, "acceptance.json"),
        JSON.stringify(
          {
            image: imageInfo.Id,
            imageReference: image,
            binary,
            binarySha256,
            manifest,
            reports,
          },
          null,
          2,
        ) + "\n",
      )
      console.log(`${scenario.init ? "init" : "non-reaping"}: ${scenario.test}: accepted`)
    }
  } finally {
    process.removeListener("SIGINT", interrupt)
    process.removeListener("SIGTERM", interrupt)
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(error)
    process.exitCode = 1
  })
}
