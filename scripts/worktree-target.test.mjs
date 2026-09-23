import assert from "node:assert/strict"
import { execFileSync, spawnSync } from "node:child_process"
import {
  chmodSync,
  copyFileSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const sourceScript =
  process.env.NESSA_WORKTREE_SCRIPT_UNDER_TEST ??
  join(dirname(fileURLToPath(import.meta.url)), "worktree.sh")

function fixture() {
  const temporary = mkdtempSync(join(tmpdir(), "nessa-worktree-target-"))
  const repo = join(temporary, "repo")
  const remote = join(temporary, "origin.git")
  const bin = join(temporary, "bin")
  mkdirSync(join(repo, "scripts"), { recursive: true })
  mkdirSync(join(repo, "src"))
  mkdirSync(bin)
  copyFileSync(sourceScript, join(repo, "scripts/worktree.sh"))
  chmodSync(join(repo, "scripts/worktree.sh"), 0o755)
  writeFileSync(
    join(repo, "Cargo.toml"),
    '[package]\nname = "artifact-owner"\nversion = "0.1.0"\nedition = "2021"\n',
  )
  writeFileSync(join(repo, "src/main.rs"), 'fn main() { println!("base"); }\n')
  writeFileSync(join(bin, "pnpm"), "#!/bin/sh\nexit 0\n")
  chmodSync(join(bin, "pnpm"), 0o755)

  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}` }
  delete env.CARGO_TARGET_DIR
  delete env.RUSTC_WRAPPER
  const git = (...args) => execFileSync("git", args, { cwd: repo, env, stdio: "pipe" })
  git("init", "-b", "main")
  git("config", "user.email", "worktree-test@example.invalid")
  git("config", "user.name", "Worktree Test")
  git("add", ".")
  git("commit", "-m", "fixture")
  execFileSync("git", ["init", "--bare", "--initial-branch=main", remote], {
    env,
    stdio: "pipe",
  })
  git("remote", "add", "origin", remote)
  git("push", "-u", "origin", "main")

  const run = (args, cwd = repo) =>
    execFileSync("bash", [join(cwd, "scripts/worktree.sh"), ...args], {
      cwd,
      env,
      encoding: "utf8",
    })
  const spawn = (args, cwd = repo, input) =>
    spawnSync("bash", [join(cwd, "scripts/worktree.sh"), ...args], {
      cwd,
      env,
      input,
      encoding: "utf8",
    })
  const spawnWithEnvironment = (args, cwd, environment) =>
    spawnSync("bash", [join(cwd, "scripts/worktree.sh"), ...args], {
      cwd,
      env: { ...env, ...environment },
      encoding: "utf8",
    })
  const pathFor = (name) => run(["path", name]).trim()

  return { temporary, repo, env, run, spawn, spawnWithEnvironment, pathFor }
}

test(
  "manual worktrees use the remote default, own artifacts, and migrate legacy links safely",
  { skip: process.platform === "win32" },
  () => {
    const context = fixture()
    const { temporary, repo, env, run, spawn, spawnWithEnvironment, pathFor } = context
    try {
      execFileSync("git", ["checkout", "-b", "parked-feature"], {
        cwd: repo,
        env,
        stdio: "pipe",
      })
      writeFileSync(join(repo, "parked-feature-only"), "must not enter new worktrees")
      execFileSync("git", ["add", "parked-feature-only"], {
        cwd: repo,
        env,
        stdio: "pipe",
      })
      execFileSync("git", ["commit", "-m", "park saved checkout"], {
        cwd: repo,
        env,
        stdio: "pipe",
      })

      run(["create", "feature/a"])
      run(["create", "feature/b"])
      const a = pathFor("feature/a")
      const b = pathFor("feature/b")
      const aTarget = join(a, "target")
      const bTarget = join(b, "target")

      const remoteMain = execFileSync("git", ["rev-parse", "origin/main"], {
        cwd: repo,
        env,
        encoding: "utf8",
      }).trim()
      for (const worktree of [a, b]) {
        assert.equal(
          execFileSync("git", ["rev-parse", "HEAD"], {
            cwd: worktree,
            env,
            encoding: "utf8",
          }).trim(),
          remoteMain,
          "manual worktree inherited the saved checkout instead of the remote default",
        )
        const upstream = spawnSync(
          "git",
          ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
          { cwd: worktree, env, encoding: "utf8" },
        )
        assert.notEqual(upstream.status, 0, "feature worktree must not track main")
      }

      writeFileSync(join(a, "src/main.rs"), 'fn main() { println!("A"); }\n')
      execFileSync("cargo", ["build", "--quiet"], { cwd: a, env })
      const executable =
        process.platform === "win32" ? "artifact-owner.exe" : "artifact-owner"
      const aBinary = join(aTarget, "debug", executable)
      assert.equal(execFileSync(aBinary, { encoding: "utf8" }).trim(), "A")

      writeFileSync(join(b, "src/main.rs"), 'fn main() { println!("B"); }\n')
      execFileSync("cargo", ["build", "--quiet"], { cwd: b, env })
      assert.equal(
        execFileSync(join(bTarget, "debug", executable), { encoding: "utf8" }).trim(),
        "B",
      )
      assert.equal(
        execFileSync(aBinary, { encoding: "utf8" }).trim(),
        "A",
        "building B must not replace the binary A's scripts resolve",
      )
      assert.equal(lstatSync(aTarget).isDirectory(), true)
      assert.equal(lstatSync(bTarget).isDirectory(), true)
      assert.notEqual(aTarget, bTarget)

      writeFileSync(join(aTarget, "owned-by-a"), "A target")
      writeFileSync(join(bTarget, "owned-by-b"), "B target")
      const refusedEnvironmentClean = spawnWithEnvironment(["clean"], b, {
        CARGO_TARGET_DIR: aTarget,
      })
      assert.notEqual(refusedEnvironmentClean.status, 0)
      assert.match(refusedEnvironmentClean.stderr, /refusing to clean/)
      assert.equal(readFileSync(join(aTarget, "owned-by-a"), "utf8"), "A target")
      assert.equal(readFileSync(join(bTarget, "owned-by-b"), "utf8"), "B target")

      mkdirSync(join(b, ".cargo"))
      writeFileSync(
        join(b, ".cargo/config.toml"),
        `[build]\ntarget-dir = ${JSON.stringify(aTarget)}\n`,
      )
      const refusedConfigClean = spawn(["clean"], b)
      assert.notEqual(refusedConfigClean.status, 0)
      assert.match(refusedConfigClean.stderr, /refusing to clean/)
      assert.equal(readFileSync(join(aTarget, "owned-by-a"), "utf8"), "A target")
      assert.equal(readFileSync(join(bTarget, "owned-by-b"), "utf8"), "B target")
      rmSync(join(b, ".cargo"), { recursive: true })

      run(["create", "legacy"])
      const legacy = pathFor("legacy")
      const legacyTarget = join(legacy, "target")
      const shared = join(repo, "target")
      rmSync(legacyTarget, { recursive: true })
      mkdirSync(shared)
      writeFileSync(join(shared, "kept"), "shared artifacts")
      const sharedPhysical = realpathSync(shared)
      symlinkSync(sharedPhysical, legacyTarget)

      const refusedClean = spawn(["clean"], legacy)
      assert.notEqual(refusedClean.status, 0)
      assert.match(refusedClean.stderr, /run 'just worktree isolate'/)
      assert.equal(readFileSync(join(shared, "kept"), "utf8"), "shared artifacts")

      run(["isolate"], legacy)
      assert.equal(lstatSync(legacyTarget).isDirectory(), true)
      assert.equal(readFileSync(join(shared, "kept"), "utf8"), "shared artifacts")

      run(["create", "foreign-link"])
      const foreign = pathFor("foreign-link")
      const foreignTarget = join(foreign, "target")
      const outside = join(temporary, "outside-target")
      rmSync(foreignTarget, { recursive: true })
      mkdirSync(outside)
      writeFileSync(join(outside, "kept"), "foreign artifacts")
      symlinkSync(outside, foreignTarget)
      const refusedForeign = spawn(["isolate"], foreign)
      assert.notEqual(refusedForeign.status, 0)
      assert.equal(readlinkSync(foreignTarget), outside)
      assert.equal(readFileSync(join(outside, "kept"), "utf8"), "foreign artifacts")

      run(["create", "remove-legacy"])
      const removable = pathFor("remove-legacy")
      rmSync(join(removable, "target"), { recursive: true })
      symlinkSync(sharedPhysical, join(removable, "target"))
      run(["remove", "remove-legacy"])
      assert.equal(readFileSync(join(shared, "kept"), "utf8"), "shared artifacts")

      const hook = spawn(["claude-hook"], repo, JSON.stringify({ name: "hook-one" }))
      assert.equal(hook.status, 0, hook.stderr)
      const hookPath = hook.stdout.trim()
      assert.equal(lstatSync(join(hookPath, "target")).isDirectory(), true)
      rmSync(join(hookPath, "target"), { recursive: true })
      symlinkSync(sharedPhysical, join(hookPath, "target"))
      const refusedReopen = spawn(
        ["claude-hook"],
        repo,
        JSON.stringify({ name: "hook-one" }),
      )
      assert.notEqual(refusedReopen.status, 0)
      assert.match(refusedReopen.stderr, /run 'just worktree isolate'/)
      assert.equal(readFileSync(join(shared, "kept"), "utf8"), "shared artifacts")
    } finally {
      rmSync(temporary, { recursive: true, force: true })
    }
  },
)
