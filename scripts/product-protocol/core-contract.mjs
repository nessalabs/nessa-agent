/** Read machine-readable bounds from the workspace's resolved sync-engine owner. */
import { spawnSync } from "node:child_process"
import { resolve } from "node:path"

function cargo(root, args) {
  const result = spawnSync("cargo", args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    env: { ...process.env, CARGO_BUILD_JOBS: "2" },
  })
  if (result.status !== 0)
    throw new Error(
      `Core wire contract command failed: ${result.error?.message ?? result.stderr}`,
    )
  return JSON.parse(result.stdout)
}

export function coreWireContract(root) {
  const metadata = cargo(root, ["metadata", "--locked", "--format-version", "1"])
  const owners = metadata.packages.filter((value) => value.name === "nessa-sync")
  if (owners.length !== 1)
    throw new Error("Wire generation requires one sync-engine owner")
  return cargo(root, [
    "run",
    "--quiet",
    "--manifest-path",
    owners[0].manifest_path,
    "--target-dir",
    resolve(metadata.target_directory, "wire-contract"),
    "--example",
    "wire_contract",
  ])
}

/** The schema names the published bound; no domain limit is implemented here. */
export function applyCoreWireBounds(value, contract) {
  if (Array.isArray(value)) {
    for (const entry of value) applyCoreWireBounds(entry, contract)
  } else if (value && typeof value === "object") {
    for (const [marker, property, encoded] of [
      ["x-core-utf8-bound", "x-utf8MaxBytes", false],
      ["x-core-maximum-bound", "maximum", false],
      ["x-core-max-items-bound", "maxItems", false],
      ["x-core-base64-bound", "maxLength", true],
    ]) {
      if (!Object.hasOwn(value, marker)) continue
      const key = value[marker]
      if (
        !Object.hasOwn(contract, key) ||
        !Number.isSafeInteger(contract[key]) ||
        contract[key] <= 0
      )
        throw new Error(`Unknown core wire bound: ${key}`)
      const bound = encoded ? 4 * Math.ceil(contract[key] / 3) : contract[key]
      if (!Number.isSafeInteger(bound))
        throw new Error(`Unrepresentable core wire bound: ${key}`)
      value[property] = bound
    }
    for (const entry of Object.values(value)) applyCoreWireBounds(entry, contract)
  }
  return value
}
