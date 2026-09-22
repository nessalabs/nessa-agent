import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { rustDependencyGraphViolations } from "./rust-dependency-graphs.mjs"

const fixture = join(
  dirname(fileURLToPath(import.meta.url)),
  "fixtures",
  "runtime-dependencies",
)

function fixtureMetadata(extraArguments = []) {
  return JSON.parse(
    execFileSync(
      "cargo",
      [
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--manifest-path",
        join(fixture, "Cargo.toml"),
        ...extraArguments,
      ],
      { encoding: "utf8", maxBuffer: 8 * 1024 * 1024 },
    ),
  )
}

/** Verify Cargo's real resolve shape for optional, renamed and transitive edges. */
export function verifyRuntimeDependencyFixture() {
  assert.deepEqual(
    rustDependencyGraphViolations(fixtureMetadata(), ["portable-runtime"]),
    [],
    "the disconnected fixture desktop framework must not taint the portable root",
  )

  const violations = rustDependencyGraphViolations(fixtureMetadata(["--all-features"]), [
    "portable-runtime",
  ])
  assert.equal(violations.length, 1)
  assert.match(violations[0], /portable-runtime.*runtime-bridge.*tauri/s)
  assert.match(violations[0], /renamed/)
}
