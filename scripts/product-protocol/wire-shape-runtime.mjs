/** Execute compiler output in an isolated generated Rust program, not another validator. */
import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { spawnSync } from "node:child_process"
import { wireFunctionName } from "./wire-shape-schema.mjs"
export function compareGeneratedRust(functions, cases) {
  const root = fileURLToPath(new URL("../../", import.meta.url))
  const version = readFileSync(resolve(root, "Cargo.lock"), "utf8").match(
    /name = "serde_json"\nversion = "([^"]+)"/,
  )[1]
  const directory = mkdtempSync(resolve(tmpdir(), "nessa-wire-shape-"))
  try {
    mkdirSync(resolve(directory, "src"))
    writeFileSync(
      resolve(directory, "Cargo.toml"),
      `[package]\nname="wire-shape-runtime-tests"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=${version}"\n`,
    )
    writeFileSync(
      resolve(directory, "src/cases.json"),
      JSON.stringify(cases.map((item) => item.input)),
    )
    writeFileSync(
      resolve(directory, "src/main.rs"),
      `use serde_json::Value;\n${functions}\nfn main() { let inputs: Vec<Value> = serde_json::from_str(include_str!("cases.json")).unwrap(); let actual = vec![${cases.map((item, index) => `${wireFunctionName(item.name)}(&inputs[${index}])`).join(",")}]; println!("{}",serde_json::to_string(&actual).unwrap()); }\n`,
    )
    const result = spawnSync(
      "cargo",
      [
        "run",
        "--offline",
        "--quiet",
        "--manifest-path",
        resolve(directory, "Cargo.toml"),
        "--target-dir",
        process.env.CARGO_TARGET_DIR ?? resolve(root, "target/wire-shape-runtime"),
      ],
      {
        encoding: "utf8",
        env: { ...process.env, CARGO_BUILD_JOBS: "2" },
        timeout: 120000,
      },
    )
    assert.equal(result.status, 0, result.stderr || result.error?.message)
    return JSON.parse(result.stdout)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}
