/** Real paired native-client coverage run alongside the window's background work. */
import {
  startProbeProcess,
  stopProbeProcess,
} from "../../../../scripts/subagent-contracts/processes.mjs"
import { repoRoot } from "./server.mjs"

export const NATIVE_CATALOGUE_TEST =
  "composition::read_only_online_tests::native_catalogue::native_paired_catalogue_stress"

async function command(args, environment, budgetMs) {
  const owned = startProbeProcess("cargo", args, {
    cwd: repoRoot,
    env: environment,
  })
  let output = ""
  let timer
  let result
  let cleanup
  try {
    result = await new Promise((resolve) => {
      timer = setTimeout(() => resolve({ timedOut: true }), budgetMs)
      const keep = (chunk) => {
        output += chunk.toString()
        if (output.length > 1024 * 1024) resolve({ overflow: true })
      }
      owned.child.stdout.on("data", keep)
      owned.child.stderr.on("data", keep)
      owned.child.on("error", (error) => resolve({ error: error.message }))
      owned.child.on("close", (code, signal) => resolve({ code, signal }))
    })
  } finally {
    clearTimeout(timer)
    cleanup = await stopProbeProcess(owned)
  }
  if (cleanup.kind !== "stopped")
    throw new Error(`native catalogue cleanup ${cleanup.kind}`)
  if (result.code !== 0)
    throw new Error(
      `native paired catalogue ${JSON.stringify(result)}\n${output.slice(-16_384)}`,
    )
  return output
}

/** Compile before opening the window; the loaded step has only its runtime budget. */
export async function preparePairedCatalogue() {
  await command(
    ["test", "--locked", "-p", "nessa-server", "--lib", "--no-run"],
    process.env,
    600_000,
  )
}

export async function pairedCatalogue(cycles) {
  const output = await command(
    [
      "test",
      "--locked",
      "-p",
      "nessa-server",
      "--lib",
      NATIVE_CATALOGUE_TEST,
      "--",
      "--exact",
      "--nocapture",
    ],
    { ...process.env, NESSA_PAIRED_CATALOGUE_CYCLES: String(cycles) },
    180_000,
  )
  if (!output.includes("test result: ok. 1 passed; 0 failed;"))
    throw new Error(
      `native paired catalogue matched no passing test\n${output.slice(-16_384)}`,
    )
  return { cycles, test: NATIVE_CATALOGUE_TEST, output }
}
