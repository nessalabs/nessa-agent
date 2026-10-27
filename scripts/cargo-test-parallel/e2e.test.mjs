import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const runner = fileURLToPath(new URL("../cargo-test-parallel.mjs", import.meta.url))
const cohortModule = "product::socket::tests::subscriptions::paired_lists::stress"
const seeds = ["seed_1", "seed_7", "seed_41", "under_load"]
const aggregate = fileURLToPath(new URL("../ci-result.mjs", import.meta.url))

// No dependencies or network: real Cargo, libtest executables, doctests, and
// subprocess exit codes. Failure switches belong only to this disposable fixture.
test("real Cargo partitions keep coverage and propagate build, harness, and doctest failures", () => {
  const root = mkdtempSync(path.join(os.tmpdir(), "nessa-ci-partition-"))
  const trace = path.join(root, "executed.txt")
  function write(name, text) {
    mkdirSync(path.dirname(path.join(root, name)), { recursive: true })
    writeFileSync(path.join(root, name), text)
  }
  const body = (id) => `
    let trace = std::env::var("NESSA_FIXTURE_TRACE").unwrap();
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().create(true).append(true).open(trace).unwrap(), "${id}").unwrap();
    assert_ne!(std::env::var("NESSA_FIXTURE_FAIL").unwrap_or_default(), "${id}");
  `
  write("Cargo.toml", '[workspace]\nmembers = ["server", "helper"]\nresolver = "2"\n')
  for (const [dir, name] of [
    ["server", "nessa-server"],
    ["helper", "helper"],
  ]) {
    write(
      `${dir}/Cargo.toml`,
      `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n`,
    )
    write(
      `${dir}/src/lib.rs`,
      `//! \`\`\`\n${body(`${dir}-doc`)
        .split("\n")
        .map((line) => `//! ${line}`)
        .join("\n")}\n//! \`\`\`\n#[test]\nfn executes() { ${body(`${dir}-lib`)} }\n`,
    )
  }
  const serverLib =
    readFileSync(path.join(root, "server/src/lib.rs"), "utf8") +
    '\n#[test] #[ignore] fn ignored_case() { panic!("ignored case executed"); }\n'
  const seededModule = (items) => `mod ${cohortModule.split("::").join(" { mod ")} {
${items.map((seed) => `#[test] fn paired_catalogue_seeded_mock_agent_stress_${seed}() { ${body(seed)} }`).join("\n")}
${"}".repeat(cohortModule.split("::").length)}`
  write("server/src/lib.rs", serverLib + seededModule(seeds))
  write(
    "server/src/main.rs",
    `fn main() {}\n#[test]\nfn executes() { ${body("server-bin")} }\n`,
  )
  const packages = ["-p", "nessa-server", "-p", "helper"]
  function run(partition, fail = "", args = packages, actions = true) {
    rmSync(trace, { force: true })
    const result = spawnSync(
      process.execPath,
      [runner, "--concurrency", "2", "--partition", partition, "--", ...args],
      {
        cwd: root,
        encoding: "utf8",
        timeout: 120_000,
        env: {
          ...process.env,
          GITHUB_ACTIONS: String(actions),
          NESSA_FIXTURE_TRACE: trace,
          NESSA_FIXTURE_FAIL: fail,
        },
      },
    )
    assert.ifError(result.error)
    return {
      ...result,
      executed: (() => {
        try {
          return readFileSync(trace, "utf8").trim().split(/\r?\n/).sort()
        } catch {
          return []
        }
      })(),
    }
  }
  function check(result, expected, status = 0) {
    assert.equal(result.status, status, result.stderr + result.stdout)
    assert.deepEqual(result.executed, [...expected].sort())
  }
  try {
    const expected = [
      "server-lib",
      "server-bin",
      "helper-lib",
      "server-doc",
      "helper-doc",
      ...seeds,
    ]
    check(run("all"), expected)
    const server = run("server-lib")
    const remaining = run("remaining")
    check(server, ["server-lib"])
    check(
      remaining,
      expected.filter((id) => id !== "server-lib"),
    )
    assert.deepEqual([...server.executed, ...remaining.executed].sort(), expected.sort())
    assert.equal(
      new Set([...server.executed, ...remaining.executed]).size,
      expected.length,
    )
    const failedServer = run("server-lib", "server-lib")
    check(failedServer, ["server-lib"], 1)
    assert.doesNotMatch(server.stderr, /::error /)
    const annotations = failedServer.stderr
      .split("\n")
      .filter((line) => line.startsWith("::error "))
    assert.equal(annotations.length, 1, failedServer.stderr)
    const message = decodeURIComponent(annotations[0].split("::").slice(2).join("::"))
    assert.match(message, /nessa_server/)
    assert.match(message, /exited with 101/)
    assert.match(message, /executes.*panicked/)
    assert.match(message, /assertion.*left != right.*failed/)
    assert.match(message, /server-lib/)
    const localFailure = run("server-lib", "server-lib", packages, false)
    check(localFailure, ["server-lib"], 1)
    assert.doesNotMatch(localFailure.stderr, /::error /)
    const failedSeed = run("remaining", "seed_1")
    assert.equal(failedSeed.status, 1)
    assert.ok(failedSeed.executed.includes("seed_1"))
    assert.ok(
      failedSeed.executed.every((id) => !id.endsWith("-doc") && id !== "server-lib"),
    )
    const failedRemaining = run("remaining", "server-bin")
    assert.equal(failedRemaining.status, 1)
    assert.ok(failedRemaining.executed.includes("server-bin"))
    assert.ok(
      failedRemaining.executed.every((id) => !id.endsWith("-doc") && id !== "server-lib"),
    )
    const failedDoc = run("remaining", "server-doc")
    assert.equal(failedDoc.status, 1)
    assert.ok(failedDoc.executed.includes("server-doc"))
    assert.match(failedDoc.stderr, /cargo test --doc exited with/)
    // The real process outcomes feed the same CLI the workflow aggregate runs.
    for (const [code, outcome, status] of [
      ["true", "success", 0],
      ["true", "failure", 1],
      ["true", "skipped", 1],
      ["false", "skipped", 0],
      ["false", "failure", 1],
    ]) {
      const result = spawnSync(process.execPath, [aggregate], {
        encoding: "utf8",
        env: {
          ...process.env,
          NEEDS: JSON.stringify({
            changes: { result: "success", outputs: { code } },
            workflows: { result: "success" },
            "local-auth": { result: remaining.status === 0 ? "success" : "failure" },
            "windows-server-tests": {
              result:
                outcome === "failure"
                  ? failedServer.status === 1
                    ? "failure"
                    : "success"
                  : outcome,
            },
          }),
        },
      })
      assert.equal(result.status, status, result.stderr)
    }
    for (const partition of ["server-lib", "remaining"]) {
      const absent = run(partition, "", ["-p", "helper"])
      check(absent, [], 1)
      assert.match(absent.stderr, /expected one nessa-server library test executable/)
    }
    write("server/src/lib.rs", serverLib + seededModule(seeds.slice(0, -1)))
    for (const part of ["server-lib", "remaining"]) {
      const missing = run(part)
      check(missing, [], 1)
      assert.match(missing.stderr, /expected exactly four seeded/)
    }
    write("server/src/lib.rs", "this is not Rust;")
    const failedBuild = run("server-lib")
    check(failedBuild, [], 1)
    assert.match(failedBuild.stderr, /cargo test --no-run exited with/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
