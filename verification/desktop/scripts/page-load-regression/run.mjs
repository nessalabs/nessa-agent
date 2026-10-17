// Compile the real diagnostic with a synthetic Tauri boundary. No native GUI,
// gateway, credentials, settings, or application data are read or written.
import { execFileSync, spawnSync } from "node:child_process"
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "../../../..")
const args = process.argv.slice(2)
if (args.length > 1 || (args.length === 1 && !args[0].startsWith("--base="))) {
  throw new Error(
    "Usage: node verification/desktop/scripts/page-load-regression/run.mjs [--base=<git-ref>]",
  )
}
const base = args[0]?.slice("--base=".length)
const source = (path) =>
  base
    ? execFileSync("git", ["show", `${base}:${path}`], { cwd: root, encoding: "utf8" })
    : readFileSync(join(root, path), "utf8")
const temporary = mkdtempSync(join(tmpdir(), "nessa-page-load-"))
try {
  const write = (path, content) => {
    const target = join(temporary, path)
    mkdirSync(dirname(target), { recursive: true })
    writeFileSync(target, content)
  }
  // The original module's privacy, module layout, and include_str! paths stay
  // intact. Append tests in its own scope; do not rewrite the implementation.
  const diagnostic = source("src-tauri/src/page_load.rs")
  write(
    "src-tauri/src/page_load.rs",
    diagnostic +
      readFileSync(join(here, "probe.rs"), "utf8").replace(
        "/* WINDOW_ARGUMENT */",
        diagnostic.includes("windows: &[&str]") ? ', &["main", "setup", "desktop"]' : "",
      ),
  )
  const hasState = diagnostic.includes("mod state;")
  if (hasState) {
    write("src-tauri/src/page_load/state.rs", source("src-tauri/src/page_load/state.rs"))
  }
  for (const file of [
    "startup-face.html",
    "startup-screen.css",
    "startup-actions.js",
    "startup-refusals.json",
  ]) {
    write(`src/host/${file}`, source(`src/host/${file}`))
  }
  write("src-tauri/src/main.rs", readFileSync(join(here, "host.rs"), "utf8"))
  const stateHarness = hasState
    ? '\n[[test]]\nname = "page_load_state"\npath = "src-tauri/src/state_main.rs"\nharness = false\n'
    : ""
  if (hasState) {
    write(
      "src-tauri/src/state_main.rs",
      readFileSync(join(here, "state_main.rs"), "utf8"),
    )
  }
  // Seed the temporary harness with the repository's dependency versions.
  const lock = source("Cargo.lock")
  write("Cargo.lock", lock)
  const dependencies = ["url", "http", "tracing", "serde_json"]
    .map((name) => {
      const version = lock.match(
        new RegExp(`name = "${name}"\\nversion = "([^"]+)"`),
      )?.[1]
      if (!version) throw new Error(`Missing ${name} in Cargo.lock`)
      return `${name} = "=${version}"`
    })
    .join("\n")
  write(
    "Cargo.toml",
    `[package]\nname = "page-load-synthetic-regression"\nversion = "0.0.0"\nedition = "2021"\n[lib]\npath = "src-tauri/src/main.rs"\n[workspace]\n[dependencies]\n${dependencies}\n[lints.rust]\nunused_imports = "deny"\nunexpected_cfgs = { level = "allow", check-cfg = ['cfg(dev)'] }\n${stateHarness}`,
  )
  const result = spawnSync(
    "cargo",
    ["test", "--manifest-path", join(temporary, "Cargo.toml")],
    { cwd: root, stdio: "inherit" },
  )
  if (result.error) throw result.error
  process.exitCode = result.status ?? 1
} finally {
  rmSync(temporary, { recursive: true, force: true })
}
