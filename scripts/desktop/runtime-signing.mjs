/**
 * How the bundled runtime's own executables get signed.
 *
 * `Contents/Resources/runtime` holds three Mach-O files that Tauri never
 * touches: `nessa`, `nessa-mcp` and the Node binary the agent harness runs on.
 * The bundler signs the app and what is inside `Contents/MacOS`; a resource is
 * a file to it, so whatever signature these arrive with is the one that ships.
 * They arrived ad-hoc, and Apple's notary service rejected v0.1.0 for it three
 * times over per binary — not a Developer ID signature, no secure timestamp,
 * no hardened runtime.
 *
 *   APPLE_SIGNING_IDENTITY set ──▶ Developer ID, --timestamp, --options runtime
 *   unset (a developer's build) ─▶ ad-hoc, exactly as before
 *
 * The identity is not looked up here. `apple-credentials.mjs` decides what a
 * build signs with and `import-certificate.mjs` puts the certificate somewhere
 * `codesign` can find it; this only turns that decision into arguments, which
 * is why it is a pure function with tests rather than a call to `security`.
 *
 * The bundled `claude` binary under `claude-acp/node_modules` is deliberately
 * not in this list. It arrives from npm already signed by Anthropic with a
 * hardened runtime, the notary log confirms it passes, and re-signing somebody
 * else's binary with our identity would replace a signature we can vouch for
 * with one we cannot.
 */

/** The executables in the runtime tree that this repository produces or ships. */
export const RUNTIME_EXECUTABLES = ["node", "nessa", "nessa-mcp"]

/**
 * Whether a binary needs entitlements, and which.
 *
 * Only Node does. V8 compiles JavaScript to native code at runtime, and the
 * hardened runtime forbids that: `allow-jit` is what permits the pages it maps
 * with `MAP_JIT`, and `allow-unsigned-executable-memory` covers the paths that
 * do not use `MAP_JIT` at all — V8 on x86_64 among them, which is half of what
 * a release builds. Without them the agent process dies the moment it starts,
 * on the machine of whoever downloaded it, and never on the one that built it.
 *
 * `nessa` and `nessa-mcp` are Rust binaries that compile nothing at runtime and
 * get none, because the shortest list that works is the right one.
 */
export function runtimeEntitlements(name) {
  return name === "node" ? "Entitlements.node.plist" : undefined
}

/**
 * The `codesign` arguments for one runtime executable.
 *
 * @param {string} path the file to sign
 * @param {object} options
 * @param {string} [options.identity] the Developer ID identity to sign with.
 *   Absent means no Apple credentials are configured — a developer's own build
 *   — and it signs ad-hoc, which is what `-` means to `codesign`.
 * @param {string} [options.entitlements] the resolved path to the entitlement
 *   plist {@link runtimeEntitlements} named, if it named one. Resolved by the
 *   caller so this stays free of the filesystem.
 * @returns {string[]} arguments, after the command name.
 */
export function signingArguments(path, { identity, entitlements } = {}) {
  if (!identity) return ["--force", "--sign", "-", path]

  // --timestamp and --options runtime are not optional extras: notarization
  // refuses a signature without a secure timestamp, and refuses an executable
  // without the hardened runtime. Each was its own error in the v0.1.0 log.
  const args = ["--force", "--sign", identity, "--timestamp", "--options", "runtime"]
  if (entitlements) args.push("--entitlements", entitlements)
  args.push(path)
  return args
}

/**
 * What is wrong with a signed runtime executable, as sentences, or nothing.
 *
 * Reads `codesign --display --verbose=2` output for the two properties Apple
 * checks and `codesign --verify` does not: an ad-hoc signature verifies
 * perfectly well, and so does a Developer ID one with the hardened runtime
 * switched off. Both were errors in the v0.1.0 notary log, and both are
 * invisible until Apple says so — twenty minutes into a build, or worse, not
 * until the release is in somebody's hands.
 *
 * @param {string} name the executable, for the message
 * @param {string} output `codesign --display --verbose=2` output (it is
 *   written to stderr, which the caller is responsible for capturing)
 * @returns {string[]} empty when the signature is one Apple would accept
 */
export function signingProblems(name, output) {
  const problems = []
  if (!/^Authority=Developer ID Application:/m.test(output))
    problems.push(
      `${name} is not signed with a Developer ID Application certificate; Apple will refuse to notarize it`,
    )
  // `flags=0x10000(runtime)` — the hardened runtime, which notarization
  // requires of every executable in the bundle.
  if (!/^CodeDirectory .*\bflags=\S*\(.*\bruntime\b.*\)/m.test(output))
    problems.push(`${name} does not have the hardened runtime enabled`)
  if (!/^Timestamp=/m.test(output))
    problems.push(`${name} has no secure timestamp; notarization requires one`)
  return problems
}
