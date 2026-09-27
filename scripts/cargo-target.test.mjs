import assert from "node:assert/strict"
import test from "node:test"
import { cargoTargetDirectory } from "./cargo-target.mjs"

test("the artifact consumer asks Cargo with the build's root and environment", () => {
  const environment = { CARGO_TARGET_DIR: "/build/owned-by-this-run" }
  let invocation
  const target = cargoTargetDirectory("/checkout", {
    environment,
    run(command, args, options) {
      invocation = { command, args, options }
      return JSON.stringify({ target_directory: environment.CARGO_TARGET_DIR })
    },
  })

  assert.equal(target, "/build/owned-by-this-run")
  assert.deepEqual(invocation, {
    command: "cargo",
    args: ["metadata", "--no-deps", "--format-version", "1"],
    options: { cwd: "/checkout", env: environment, encoding: "utf8" },
  })
})

test("an absent target answer is refused instead of falling back to stale output", () => {
  assert.throws(
    () =>
      cargoTargetDirectory("/checkout", {
        run: () => JSON.stringify({ packages: [] }),
      }),
    /did not report a target directory/,
  )
})
