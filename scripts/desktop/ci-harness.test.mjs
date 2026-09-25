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
  // The database opener's privacy checks answer differently on each OS, so
  // they run in the matrix, as `pnpm check` runs them locally.
  assert.match(root.scripts.check, /pnpm database:check/)
  assert.match(workflow, /run: cargo test -p nessa-local-database/)
  assert.match(
    workflow,
    /run: cargo clippy -p nessa-local-database --all-targets -- -D warnings/,
  )
  assert.match(workflow, /npm ci --ignore-scripts/)
})

test("the existing Linux matrix leg uniquely owns real runtime assembly", () => {
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  assert.equal(workflow.match(/run: node scripts\/desktop\/prepare\.mjs/g)?.length, 1)
  assert.match(
    workflow,
    /name: Assemble the Linux desktop runtime\s+if: runner\.os == 'Linux'\s+run: node scripts\/desktop\/prepare\.mjs/,
  )
  const releaseWorkflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.doesNotMatch(releaseWorkflow, /target: x86_64-unknown-linux-gnu/)
  assert.doesNotMatch(releaseWorkflow, /updater-target: linux-/)
})

test("the disposable user manager proves the same session bus and cleans its exact runtime", () => {
  const script = readFileSync("scripts/desktop/check-systemd-user-service.sh", "utf8")
  assert.match(script, /systemctl --user show-environment/)
  assert.match(script, /systemd-run --user --pipe --wait --collect --quiet/)
  assert.match(script, /--unit="\$transient_unit"/)
  assert.match(script, /-p Delegate=yes -p Type=exec -d/)
  assert.match(script, /export XDG_RUNTIME_DIR="\$RUN_DIR"/)
  assert.match(script, /export XDG_CONFIG_HOME="\$RUN_DIR\/config"/)
  assert.match(script, /export XDG_DATA_HOME="\$RUN_DIR\/data"/)
  assert.match(script, /export XDG_STATE_HOME="\$RUN_DIR\/state"/)
  assert.match(script, /export XDG_CACHE_HOME="\$RUN_DIR\/cache"/)
  assert.match(
    script,
    /export SYSTEMD_ENVIRONMENT_GENERATOR_PATH="\$RUN_DIR\/empty-environment-generators"/,
  )
  assert.match(script, /export SYSTEMD_GENERATOR_PATH="\$RUN_DIR\/empty-generators"/)
  assert.match(script, /export SYSTEMD_UNIT_PATH="\$RUN_DIR\/systemd\/user:/)
  assert.match(script, /systemd --user --unit=basic\.target/)
  assert.match(script, /disposable systemd user manager ready at/)
  assert.match(script, /disposable systemd gateway lifecycle completed/)
  assert.match(script, /SYSTEMD_LOG_LEVEL=debug SYSTEMD_LOG_TARGET=console/)
  assert.match(script, /kill -0 "\$manager_pid"/)
  assert.match(script, /manager exited before readiness with status \$manager_status/)
  assert.match(script, /-S "\$XDG_RUNTIME_DIR\/systemd\/private"/)
  assert.match(script, /busctl --address="\$DBUS_SESSION_BUS_ADDRESS"/)
  assert.match(script, /status org\.freedesktop\.systemd1/)
  assert.match(script, /2>"\$bus_error"/)
  assert.doesNotMatch(script, /busctl --user/)
  assert.doesNotMatch(script, /loginctl|enable-linger/)
  assert.match(
    script,
    /inaccessible_directory="\$runtime_directory\/systemd\/inaccessible\/dir"/,
  )
  assert.match(script, /chmod 700 "\$inaccessible_directory"/)
  assert.doesNotMatch(script, /chmod -R|find .* -delete/)
  assert.match(script, /primary_status=\$\?/)
  assert.match(script, /if \[ "\$primary_status" -eq 0 \]/)
  assert.equal(script.match(/print_manager_diagnostics/g)?.length, 3)
})

test("the frontend job owns top-level script tests and their just dependency", () => {
  const root = JSON.parse(readFileSync("package.json", "utf8"))
  const workflow = readFileSync(".github/workflows/local-auth.yml", "utf8")
  const gatewayStart = workflow.indexOf("  gateway-contract:")
  const releaseStart = workflow.indexOf("  desktop-release-profile:")
  const frontendStart = workflow.indexOf("  frontend:")
  const localAuthStart = workflow.indexOf("  local-auth:")
  for (const boundary of [gatewayStart, releaseStart, frontendStart, localAuthStart])
    assert.notEqual(boundary, -1, "expected CI job boundary is missing")
  const gateway = workflow.slice(gatewayStart, releaseStart)
  const frontend = workflow.slice(frontendStart, localAuthStart)

  assert.equal(root.scripts["scripts:test"], "node --test scripts/*.test.mjs")
  assert.ok(root.scripts["frontend:check"].includes("pnpm scripts:test"))
  assert.doesNotMatch(workflow, /run: node --test scripts\/\*\.test\.mjs/)
  assert.doesNotMatch(gateway, /node --test scripts\/\*\.test\.mjs/)
  assert.doesNotMatch(gateway, /install-action@just/)
  assert.equal(workflow.match(/install-action@just/g)?.length, 1)
  assert.match(
    frontend,
    /install-action@just[\s\S]*run: pnpm frontend:check/,
    "the suite's just prerequisite must be installed before its owning aggregate",
  )
})

test("nothing in a release is built or published before the key pairing gate", () => {
  // The one release failure with no remedy: ship a build whose trusted public
  // key is not the other half of the signing key, and every update it will ever
  // see is rejected forever. The gate that settles it must stay ahead of the
  // builds, and must stay in its demanding mode — its default when no key is
  // reachable is a *skip*, which in a release would read as a pass.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(
    workflow,
    /^\s*- run: cargo test -p nessa-app --test updater_key_pairing --no-default-features\s*$/m,
  )
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
  // publishes it. Publishing is the deliberate act that offers the update, and
  // it stays a person's.
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /gh release create "\$TAG" --draft/)
  assert.doesNotMatch(workflow, /--draft=false|gh release edit .*--draft/)
})

/**
 * The notes say where to look, rather than asserting a fact about signing.
 *
 * They used to state flatly that the artifacts were not notarized, which was
 * true when nothing had ever been built and false the moment the secrets were
 * set — and nothing would have corrected it. What is actually true of every
 * build is that the build says which of the two it did.
 */
test("the draft's notes point at the build's own account of what it signed with", () => {
  const workflow = readFileSync(".github/workflows/release.yml", "utf8")
  assert.match(workflow, /Choose what this build signs with/)
  assert.doesNotMatch(
    workflow,
    /\*\*These artifacts are not notarized\.\*\*/,
    "the notes state something about signing that no build checked",
  )
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
