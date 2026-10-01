// When scripts/remote/macos-app.sh reuses a downloaded host and when it asks CI
// for a new one (#358). Each case builds a throwaway repository, records a build
// keyed on that tree, edits something, and watches whether `gh` is reached: the
// stub stands in for the push-and-build path and fails loudly when called.
import { test } from "node:test"
import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import { chmodSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const script = join(dirname(fileURLToPath(import.meta.url)), "macos-app.sh")

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "macos-app-"))
  const repo = join(root, "repo")
  const write = (path, text) => {
    mkdirSync(dirname(join(repo, path)), { recursive: true })
    writeFileSync(join(repo, path), text)
  }
  const git = (...args) => execFileSync("git", args, { cwd: repo, encoding: "utf8" })
  mkdirSync(repo)
  git("init", "-q")
  git(
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@t",
    "commit",
    "-q",
    "--allow-empty",
    "-m",
    "base",
  )
  write("src-tauri/src/main.rs", "fn main() {}\n")
  write("src-tauri/src/with space.rs", "\n")
  write("src-tauri/capabilities/default.json", "{}\n")
  write("src/app.tsx", "export {}\n")

  const bin = join(root, "bin")
  mkdirSync(bin)
  writeFileSync(join(bin, "gh"), "#!/bin/sh\necho 'gh reached' >&2\nexit 97\n")
  chmodSync(join(bin, "gh"), 0o755)
  const env = {
    ...process.env,
    XDG_CACHE_HOME: join(root, "cache"),
    PATH: `${bin}:${process.env.PATH}`,
  }
  const run = (...args) =>
    spawnSync("bash", [script, ...args], { cwd: repo, env, encoding: "utf8" })

  // A build of the current tree whose inputs are two source files, one with a
  // space in its path, and a watched directory.
  const lines =
    "src-tauri/capabilities\nsrc-tauri/src/main.rs\nsrc-tauri/src/with space.rs\n"
  const record = () => {
    const inputs = join(root, "inputs.txt")
    writeFileSync(inputs, lines)
    const key = run("--key", inputs).stdout.trim()
    const build = join(root, "cache", "nessa", "macos-app", key)
    mkdirSync(build, { recursive: true })
    writeFileSync(join(build, "inputs.txt"), lines)
    writeFileSync(join(build, "nessa-app"), "")
    return join(build, "nessa-app")
  }
  return {
    root,
    write,
    run,
    record,
    done: () => rmSync(root, { recursive: true, force: true }),
  }
}

test("an unchanged tree reuses the recorded build", () => {
  const f = fixture()
  try {
    const binary = f.record()
    const result = f.run()
    assert.equal(result.status, 0, result.stderr)
    assert.equal(result.stdout.trim(), binary)
  } finally {
    f.done()
  }
})

test("an edit outside the inputs, uncommitted and untracked, reuses the build", () => {
  const f = fixture()
  try {
    const binary = f.record()
    f.write("src/app.tsx", "export const changed = 1\n")
    f.write("src/new.tsx", "export {}\n")
    const result = f.run()
    assert.equal(result.status, 0, result.stderr)
    assert.equal(result.stdout.trim(), binary)
  } finally {
    f.done()
  }
})

test("an edit to an input file asks CI for a new build", () => {
  const f = fixture()
  try {
    f.record()
    f.write("src-tauri/src/main.rs", "fn main() { println!() }\n")
    const result = f.run()
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, /gh reached/)
  } finally {
    f.done()
  }
})

for (const [what, path, text] of [
  ["a file added to a watched directory", "src-tauri/capabilities/extra.json", "{}\n"],
  [
    "an edit to an input whose path has a space",
    "src-tauri/src/with space.rs",
    "// edited\n",
  ],
  ["a build script no build listed yet", "crates/new/build.rs", "fn main() {}\n"],
  ["a toolchain file no build listed yet", "rust-toolchain.toml", "[toolchain]\n"],
]) {
  test(`${what} asks CI for a new build`, () => {
    const f = fixture()
    try {
      f.record()
      f.write(path, text)
      const result = f.run()
      assert.notEqual(result.status, 0)
      assert.match(result.stderr, /gh reached/)
    } finally {
      f.done()
    }
  })
}

test("dep-info inputs are the root's files, relative, with escaped spaces and .. resolved", () => {
  const root = mkdtempSync(join(tmpdir(), "depinfo-"))
  try {
    const depinfo = join(root, "nessa-app.d")
    writeFileSync(
      depinfo,
      `${root}/target/debug/nessa-app: ${root}/src-tauri/src/main.rs ${root}/src-tauri/src/with\\ space.rs \\\n` +
        ` ${root}/crates/a/src/../../../protocol/defaults/ports.json /home/x/.cargo/registry/serde/lib.rs\n\n` +
        `${root}/src-tauri/src/main.rs:\n`,
    )
    const inputs = execFileSync(
      "python3",
      [script.replace("macos-app.sh", "host_inputs.py"), "from-depinfo", depinfo, root],
      {
        encoding: "utf8",
      },
    )
    assert.deepEqual(inputs.trim().split("\n"), [
      "protocol/defaults/ports.json",
      "src-tauri/src/main.rs",
      "src-tauri/src/with space.rs",
    ])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
