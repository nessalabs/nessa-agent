#!/usr/bin/env node
/** Enforce framework-free Cargo resolve graphs for reusable Rust packages. */
import { execFileSync } from "node:child_process"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import {
  PORTABLE_RUST_PACKAGES,
  rustDependencyGraphViolations,
} from "./architecture/rust-dependency-graphs.mjs"
import { verifyRuntimeDependencyFixture } from "./architecture/runtime-dependencies-fixture.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")

function metadata(extraArguments = []) {
  return JSON.parse(
    execFileSync(
      "cargo",
      ["metadata", "--locked", "--format-version", "1", ...extraArguments],
      { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    ),
  )
}

if (process.argv.includes("--verify-fixture")) verifyRuntimeDependencyFixture()

const configurations = [
  ["default features", []],
  ["all features", ["--all-features"]],
]
const failures = []
for (const [configuration, arguments_] of configurations) {
  for (const violation of rustDependencyGraphViolations(
    metadata(arguments_),
    PORTABLE_RUST_PACKAGES,
  ))
    failures.push(`${configuration}: ${violation}`)
}

if (failures.length > 0) {
  console.error("portable Rust dependency check failed:\n")
  for (const failure of failures) console.error(`  ${failure}`)
  process.exit(1)
}

console.log(
  `portable Rust dependency check passed for ${PORTABLE_RUST_PACKAGES.join(", ")} (default and all features)`,
)
