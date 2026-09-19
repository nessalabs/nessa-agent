import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("local-auth integration harness is locked to its declared tools", () => {
  const manifest = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package.json", "utf8"),
  )
  const lock = JSON.parse(
    readFileSync(".github/harnesses/local-auth/package-lock.json", "utf8"),
  )
  assert.deepEqual(lock.packages[""].dependencies, manifest.dependencies)
  assert.deepEqual(manifest.dependencies, {
    "@nessa/client": "file:./packages/nessa-client",
    tsx: "4.23.13",
    ws: "8.18.3",
  })
  assert.deepEqual(lock.packages["node_modules/@nessa/client"], {
    resolved: "packages/nessa-client",
    link: true,
  })
  for (const dependency of ["tsx", "ws"]) {
    const installed = lock.packages[`node_modules/${dependency}`]
    assert.equal(installed.version, manifest.dependencies[dependency])
    assert.match(installed.integrity, /^sha512-/)
  }
})

test("local and CI aggregate the same named frontend and native checks", () => {
  const root = JSON.parse(readFileSync("package.json", "utf8"))
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.match(root.scripts.check, /pnpm frontend:check/)
  for (const [aggregate, script, command] of [
    ["check", "frontend:check", "pnpm frontend:check"],
    ["sdk:check", "sdk:docs:check", "node scripts/check-sdk-docs.mjs"],
    ["check", "mcp:check", "node scripts/check-mcp.mjs"],
    ["check", "desktop:check", "node scripts/check-desktop.mjs"],
  ]) {
    assert.ok(root.scripts[aggregate].includes(`pnpm ${script}`))
    assert.ok(workflow.includes(`run: ${command}`), `CI is missing ${script}`)
  }
  assert.match(
    workflow,
    /cargo fmt -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk -- --check/,
  )
  assert.equal(
    root.scripts.architecture,
    "node --test scripts/architecture/*.test.mjs && node scripts/check-architecture.mjs",
  )
  assert.match(workflow, /node --test scripts\/architecture\/\*\.test\.mjs/)
  assert.match(workflow, /node scripts\/check-architecture\.mjs/)
  assert.match(
    workflow,
    /cargo test -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk/,
  )
  assert.match(
    workflow,
    /cargo clippy -p nessa-local-storage -p nessa-auth -p nessa-server -p nessa-sdk --all-targets -- -D warnings/,
  )
  assert.match(workflow, /npm ci --ignore-scripts/)
})

test("nothing in a release is built or published before the key pairing gate", () => {
  // The one release failure with no remedy: ship a build whose trusted public
  // key is not the other half of the signing key, and every update it will ever
  // see is rejected forever. The gate that settles it must stay ahead of the
  // builds, and must stay in its demanding mode — its default when no key is
  // reachable is a *skip*, which in a release would read as a pass.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /cargo test -p nessa-app --test updater_key_pairing/)
  assert.match(workflow, /NESSA_REQUIRE_UPDATER_KEY_PAIRING: "1"/)
  assert.match(workflow, /needs: \[version, updater-key\]/)
  assert.match(workflow, /needs: \[version, build\]/)
})

test("a release builds both macOS architectures and only macOS bundles", () => {
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  // Separate runners, because prepare-macos.mjs cannot cross-compile and there
  // is no universal build. Both must reach the same manifest.
  for (const target of ["aarch64-apple-darwin", "x86_64-apple-darwin"])
    assert.ok(workflow.includes(`target: ${target}`), `no release build for ${target}`)
  // The shipped config says "all" and stays that way for local builds; a release
  // narrows it, because a .deb or an NSIS installer would launch and then be
  // unable to run an agent at all.
  const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"))
  assert.equal(config.bundle.targets, "all")
  assert.match(workflow, /--bundles app,dmg/)
})

test("a release is staged as a draft, never published by the workflow", () => {
  // `releases/latest/download/latest.json` resolves only to a published,
  // non-prerelease release, so a draft offers nothing to anyone until a person
  // publishes it. While artifacts are unnotarized that step must stay manual.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /gh release create "\$TAG" --draft/)
  assert.doesNotMatch(workflow, /--draft=false|gh release edit .*--draft/)
  assert.match(workflow, /not notarized/i)
})

test("a release builds the commit its tag names, not a branch of the same name", () => {
  // `actions/checkout` takes an unqualified ref and looks for a branch before a
  // tag, so a branch called `v0.1.0` would be what got built. Comparing declared
  // versions cannot catch it: both commits can say the same version. The tag is
  // resolved to a commit once, and every job builds that commit.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /git\/ref\/tags\//)
  assert.doesNotMatch(
    workflow,
    /ref: \$\{\{ (needs\.version\.outputs\.tag|steps\.resolve\.outputs\.tag) \}\}/,
    "a job is still checking out a name rather than the resolved commit",
  )
  for (const match of workflow.matchAll(/ref: \$\{\{ ([^}]+) \}\}/g))
    assert.match(match[1], /\.sha\b/, `checkout of ${match[1].trim()} is not a commit`)
})

test("publication requires the tag to exist and states the channel", () => {
  // `gh release create` creates a missing tag from the default branch, which
  // would publish code nobody tagged; `--verify-tag` refuses instead. And
  // GitHub keeps pre-release apart from the tag's spelling, so an rc published
  // without the flag is an ordinary release — the one the updater serves.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /gh release create "\$TAG" --draft --verify-tag/)
  assert.match(workflow, /\$\{PRERELEASE:\+--prerelease\}/)
  // A draft from a run before the channel was set is corrected rather than left.
  assert.match(workflow, /gh release edit "\$TAG" --prerelease=/)
})
