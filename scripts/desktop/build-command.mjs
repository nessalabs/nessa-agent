import { LINUX_RUNTIME_TARGET } from "./prepare-linux.mjs"
import { releaseBundles } from "./release-assets.mjs"
import { desktopStageEnvironment, resolveDesktopStage } from "./stage.mjs"

function optionValue(args, longName, shortName) {
  const matches = []
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index]
    if (argument === longName || argument === shortName) {
      const value = args[index + 1]
      if (!value || value.startsWith("-")) throw new Error(`${longName} requires a value`)
      matches.push(value)
      index += 1
    } else if (argument.startsWith(`${longName}=`)) {
      const value = argument.slice(longName.length + 1)
      if (!value) throw new Error(`${longName} requires a value`)
      matches.push(value)
    } else if (shortName && argument.startsWith(`${shortName}=`)) {
      const value = argument.slice(shortName.length + 1)
      if (!value) throw new Error(`${longName} requires a value`)
      matches.push(value)
    } else if (
      shortName &&
      argument.startsWith(shortName) &&
      argument.length > shortName.length
    ) {
      matches.push(argument.slice(shortName.length))
    }
  }
  if (matches.length > 1) throw new Error(`${longName} may be specified only once`)
  return matches[0]
}

function withoutOption(args, longName) {
  const forwarded = []
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index]
    if (argument === longName) {
      index += 1
    } else if (!argument.startsWith(`${longName}=`)) {
      forwarded.push(argument)
    }
  }
  return forwarded
}

/** The check each platform's built bundle must pass. A platform without one
 * (Windows) returns once the build succeeds. */
const BUNDLE_VERIFIERS = {
  darwin: "scripts/desktop/verify-macos-bundle.mjs",
  linux: "scripts/desktop/verify-linux-bundle.mjs",
}

/** Whether Tauri would read a choice of bundles from these arguments:
 * `--bundles` in any form, `--no-bundle`, or `-b` alone or inside a cluster of
 * short flags (`-db`). It is given Tauri's arguments only
 * (`splitRunnerArguments`).
 *
 * It errs toward refusing: a `b` inside a value attached to another short flag
 * (`-c'{"bundle":…}'`) reads as `-b` too, so pass such a value as
 * `--config <value>`. A refusal happens before compiling and makes nothing;
 * the opposite error would build a bundle nobody checks. */
export function choosesBundles(args) {
  return args.some(
    (argument) =>
      argument === "--no-bundle" ||
      argument === "--bundles" ||
      argument.startsWith("--bundles=") ||
      /^-[^-]*b/.test(argument),
  )
}

/** The arguments before `--`, which are Tauri's and this command's, and the
 * runner's arguments from `--` on. Everything that reads options reads the
 * first part only. */
export function splitRunnerArguments(args) {
  const start = args.indexOf("--")
  return start === -1
    ? { own: args, runner: [] }
    : { own: args.slice(0, start), runner: args.slice(start) }
}

/** Parse the Tauri arguments that select the artifact verified after a build. */
export function parseBuildArguments(args) {
  return {
    stage: optionValue(args, "--stage"),
    target: optionValue(args, "--target", "-t"),
    bundles: optionValue(args, "--bundles", "-b"),
  }
}

/** Run the public desktop build and verify the artifact selected by its arguments. */
export function runDesktopBuild({ args, environment, platform, spawn, now = Date.now }) {
  const { own, runner: runnerArgs } = splitRunnerArguments(args)
  const parsed = parseBuildArguments(own)
  const { stage: requestedStage, target } = parsed
  // A Linux build makes exactly what a Linux release makes, and nothing a
  // caller names: the verifier checks the .deb alone, so any other choice of
  // bundles is refused here, before minutes of compiling, rather than parsed.
  if (platform === "linux" && choosesBundles(own))
    throw new Error(
      "A Linux build makes the release's bundles; drop --bundles and --no-bundle",
    )
  const bundles =
    parsed.bundles ??
    (platform === "linux" ? releaseBundles(target ?? LINUX_RUNTIME_TARGET) : undefined)
  const stage = resolveDesktopStage({
    environment,
    fallback: "prod",
    requested: requestedStage,
  })
  const buildEnvironment = desktopStageEnvironment(environment, stage)
  // Tauri's own options go before any `--`: what follows it is the runner's.
  const tauriArgs = withoutOption(own, "--stage")
  if (bundles && !parsed.bundles) tauriArgs.push("--bundles", bundles)
  if (platform === "darwin" || platform === "linux") {
    const bundle = { resources: { "runtime/": "runtime/" } }
    if (platform === "darwin") {
      const identity = environment.APPLE_SIGNING_IDENTITY?.trim() || "-"
      bundle.macOS = { signingIdentity: identity }
    }
    tauriArgs.push("--config", JSON.stringify({ bundle }))
  }
  const command = ["exec", "tauri", "build", ...tauriArgs, ...runnerArgs]

  const pnpm = platform === "win32" ? "pnpm.cmd" : "pnpm"
  const started = now()
  const build = spawn(pnpm, command, { env: buildEnvironment, stdio: "inherit" })
  if (build.error) throw build.error
  if (build.status !== 0) return build.status ?? 1
  if (!Object.hasOwn(BUNDLE_VERIFIERS, platform)) return 0

  const verificationEnvironment = { ...buildEnvironment }
  delete verificationEnvironment.NESSA_BUILD_TARGET
  delete verificationEnvironment.NESSA_BUILD_BUNDLES
  if (target) verificationEnvironment.NESSA_BUILD_TARGET = target
  if (bundles) verificationEnvironment.NESSA_BUILD_BUNDLES = bundles
  // When the build began, so the verifier checks the package this build
  // wrote rather than one an earlier build left under another version.
  verificationEnvironment.NESSA_BUILD_STARTED = String(started)
  if (platform === "darwin") {
    // Between the build and the verification, because the bundler notarizes
    // the app and then builds the disk image around it: the image itself has
    // no ticket, and it is the artifact a person downloads. Does nothing when
    // the build did not notarize, or produced no disk image.
    const staple = spawn("node", ["scripts/desktop/notarize-disk-image.mjs"], {
      env: verificationEnvironment,
      stdio: "inherit",
    })
    if (staple.error) throw staple.error
    if (staple.status !== 0) return staple.status ?? 1
  }

  const verify = spawn("node", [BUNDLE_VERIFIERS[platform]], {
    env: verificationEnvironment,
    stdio: "inherit",
  })
  if (verify.error) throw verify.error
  return verify.status ?? 1
}
