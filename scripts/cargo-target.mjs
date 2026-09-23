import { execFileSync } from "node:child_process"

/** Ask Cargo where this invocation writes artifacts, including explicit overrides. */
export function cargoTargetDirectory(
  root,
  { environment = process.env, run = execFileSync } = {},
) {
  const output = run("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: root,
    env: environment,
    encoding: "utf8",
  })
  const target = JSON.parse(output).target_directory
  if (typeof target !== "string" || target.length === 0)
    throw new Error("cargo metadata did not report a target directory")
  return target
}
