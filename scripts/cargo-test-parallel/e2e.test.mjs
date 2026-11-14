import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { spawnSync } from "node:child_process"
import {
  closeSync,
  mkdtempSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import os from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { testExecutables } from "../cargo-test-parallel.mjs"
import { withFailureAnnotations } from "./diagnostics.mjs"

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
    let fail = std::env::var("NESSA_FIXTURE_FAIL").unwrap_or_default();
    if fail == "${id}" { eprintln!("fixture context: {}", "x".repeat(40_000)); }
    assert_ne!(fail, "${id}");
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
    '\n#[test] #[ignore] fn ignored_case() { panic!("ignored case executed"); }\n' +
    `mod conversation { #[test] fn executes() { ${body("server-conversation")} } }\n`
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
      "server-conversation",
      "server-bin",
      "helper-lib",
      "server-doc",
      "helper-doc",
      ...seeds,
    ]
    check(run("all"), expected)
    const conversation = run("server-conversation")
    const other = run("server-other")
    const remaining = run("remaining")
    check(conversation, ["server-conversation"])
    check(other, ["server-lib"])
    check(
      remaining,
      expected.filter((id) => id !== "server-lib" && id !== "server-conversation"),
    )
    assert.deepEqual(
      [...conversation.executed, ...other.executed, ...remaining.executed].sort(),
      expected.sort(),
    )
    assert.equal(
      new Set([...conversation.executed, ...other.executed, ...remaining.executed]).size,
      expected.length,
    )
    const failedServer = run("server-other", "server-lib")
    check(failedServer, ["server-lib"], 1)
    assert.doesNotMatch(other.stderr, /::error /)
    const annotations = failedServer.stderr
      .split("\n")
      .filter((line) => line.startsWith("::error "))
    assert.ok(
      annotations.length > 1,
      "the real assertion follows more than one receiver-sized chunk",
    )
    const parts = annotations.map((line) => {
      const decoded = decodeURIComponent(line.split("::").slice(2).join("::"))
      assert.ok(
        decoded.length <= 3500,
        "complete messages survive the 4096-character receiver",
      )
      const received = decoded.slice(0, 4096)
      const header = /^Retained report ([a-f0-9]{64}) part (\d+)\/(\d+)\n/.exec(received)
      assert.ok(header)
      return {
        hash: header[1],
        index: Number(header[2]),
        count: Number(header[3]),
        data: received.slice(header[0].length),
      }
    })
    assert.equal(parts[0].index, parts.length)
    assert.match(parts[0].data, /assertion.*left != right.*failed/)
    assert.ok(
      parts.every((part) => part.hash === parts[0].hash && part.count === parts.length),
    )
    assert.equal(new Set(parts.map((part) => part.index)).size, parts.length)
    const message = parts
      .sort((a, b) => a.index - b.index)
      .map((part) => part.data)
      .join("")
    assert.equal(createHash("sha256").update(message).digest("hex"), parts[0].hash)
    assert.match(message, /nessa_server/)
    assert.match(message, /exited with 101/)
    assert.match(message, /executes.*panicked/)
    assert.match(message, /assertion.*left != right.*failed/)
    assert.match(message, /server-lib/)
    const localFailure = run("server-other", "server-lib", packages, false)
    check(localFailure, ["server-lib"], 1)
    assert.doesNotMatch(localFailure.stderr, /::error /)
    const failedSeed = run("remaining", "seed_1")
    assert.equal(failedSeed.status, 1)
    assert.ok(failedSeed.executed.includes("seed_1"))
    assert.ok(
      failedSeed.executed.every(
        (id) =>
          !id.endsWith("-doc") && id !== "server-lib" && id !== "server-conversation",
      ),
    )
    const failedRemaining = run("remaining", "server-bin")
    assert.equal(failedRemaining.status, 1)
    assert.ok(failedRemaining.executed.includes("server-bin"))
    assert.ok(
      failedRemaining.executed.every(
        (id) =>
          !id.endsWith("-doc") && id !== "server-lib" && id !== "server-conversation",
      ),
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
    for (const partition of ["server-conversation", "server-other", "remaining"]) {
      const absent = run(partition, "", ["-p", "helper"])
      check(absent, [], 1)
      assert.match(absent.stderr, /expected one nessa-server library test executable/)
    }
    write("server/src/lib.rs", serverLib + seededModule(seeds.slice(0, -1)))
    for (const part of ["server-conversation", "server-other", "remaining"]) {
      const missing = run(part)
      check(missing, [], 1)
      assert.match(missing.stderr, /expected exactly four seeded/)
    }
    write("server/src/lib.rs", "this is not Rust;")
    const failedBuild = run("server-conversation")
    check(failedBuild, [], 1)
    assert.match(failedBuild.stderr, /cargo test --no-run exited with/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

// Both descriptors share one file offset, so the fixture proves ordering rather
// than depending on when Node happens to drain separate stdout/stderr pipes.
test("default libtest capture retains an early assertion after a later noisy success", async () => {
  const root = mkdtempSync(path.join(os.tmpdir(), "nessa-ci-capture-"))
  try {
    mkdirSync(path.join(root, "src"))
    writeFileSync(
      path.join(root, "Cargo.toml"),
      '[package]\nname="capture-evidence"\nversion="0.1.0"\nedition="2021"\n[workspace]\n',
    )
    writeFileSync(
      path.join(root, "src/lib.rs"),
      `
#[test] fn a_early_failure() { panic!("CONTROL_EARLY_ASSERTION"); }
#[test] fn z_later_noisy_success() { println!("LATER_SUCCESS_CONTEXT {}", "x".repeat(40_000)); }
`,
    )
    const environment = { ...process.env, CARGO_TARGET_DIR: path.join(root, "target") }
    const build = spawnSync(
      "cargo",
      ["test", "--no-run", "--offline", "--message-format=json"],
      { cwd: root, env: environment, encoding: "utf8", timeout: 120_000 },
    )
    assert.ifError(build.error)
    assert.equal(build.status, 0, build.stderr)
    const executables = testExecutables(build.stdout)
    assert.equal(executables.length, 1)
    for (const uncaptured of [true, false]) {
      const outputFile = path.join(root, `output-${uncaptured}.txt`)
      const descriptor = openSync(outputFile, "w")
      const env = { ...environment }
      delete env.RUST_TEST_NOCAPTURE
      if (uncaptured) env.RUST_TEST_NOCAPTURE = "1"
      let result
      try {
        result = spawnSync(executables[0].executable, ["--test-threads=1"], {
          cwd: root,
          env,
          stdio: ["ignore", descriptor, descriptor],
          timeout: 120_000,
        })
      } finally {
        closeSync(descriptor)
      }
      assert.ifError(result.error)
      assert.equal(result.status, 101)
      const output = readFileSync(outputFile, "utf8")
      assert.match(output, /test result: FAILED\. 1 passed; 1 failed;/)
      assert.match(output, /CONTROL_EARLY_ASSERTION/)
      assert.ok(
        output.indexOf("test a_early_failure") <
          output.indexOf("test z_later_noisy_success"),
      )
      assert.equal(output.includes("LATER_SUCCESS_CONTEXT"), uncaptured)
      if (uncaptured)
        assert.ok(
          output.indexOf("CONTROL_EARLY_ASSERTION") <
            output.indexOf("LATER_SUCCESS_CONTEXT"),
        )
      const annotations = []
      let forwarded = ""
      const code = await withFailureAnnotations(
        async (_command, _args, io) => {
          io.onStdout(output)
          return result.status
        },
        (line) => annotations.push(line),
      )(executables[0].executable, ["--test-threads=1"], {
        onStdout: (chunk) => {
          forwarded += chunk
        },
      })
      assert.equal(code, 101)
      assert.equal(forwarded, output)
      const evidence = annotations
        .map((line) =>
          decodeURIComponent(line.split("::").slice(2).join("::").slice(0, -1)),
        )
        .join("\n")
      assert.equal(evidence.includes("CONTROL_EARLY_ASSERTION"), !uncaptured)
      assert.equal(annotations.length > 1, uncaptured)
    }
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
