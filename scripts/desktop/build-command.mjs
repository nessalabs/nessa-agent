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

/** Parse the Tauri arguments that select the artifact verified after a build. */
export function parseBuildArguments(args) {
  return {
    stage: optionValue(args, "--stage"),
    target: optionValue(args, "--target", "-t"),
    bundles: optionValue(args, "--bundles", "-b"),
  }
}

/** Run the public desktop build and verify the artifact selected by its arguments. */
export function runDesktopBuild({ args, environment, platform, spawn }) {
  const { stage: requestedStage, target, bundles } = parseBuildArguments(args)
  const stage = resolveDesktopStage({
    environment,
    fallback: "prod",
    requested: requestedStage,
  })
  const buildEnvironment = desktopStageEnvironment(environment, stage)
  const command = ["exec", "tauri", "build", ...withoutOption(args, "--stage")]
  if (platform === "darwin" || platform === "linux") {
    const bundle = { resources: { "runtime/": "runtime/" } }
    if (platform === "darwin") {
      const identity = environment.APPLE_SIGNING_IDENTITY?.trim() || "-"
      bundle.macOS = { signingIdentity: identity }
    }
    command.push("--config", JSON.stringify({ bundle }))
  }

  const pnpm = platform === "win32" ? "pnpm.cmd" : "pnpm"
  const build = spawn(pnpm, command, { env: buildEnvironment, stdio: "inherit" })
  if (build.error) throw build.error
  if (build.status !== 0) return build.status ?? 1
  if (platform !== "darwin") return 0

  const verificationEnvironment = { ...buildEnvironment }
  delete verificationEnvironment.NESSA_BUILD_TARGET
  delete verificationEnvironment.NESSA_BUILD_BUNDLES
  if (target) verificationEnvironment.NESSA_BUILD_TARGET = target
  if (bundles) verificationEnvironment.NESSA_BUILD_BUNDLES = bundles
  // Between the build and the verification, because the bundler notarizes the
  // app and then builds the disk image around it: the image itself has no
  // ticket, and it is the artifact a person downloads. Does nothing when the
  // build did not notarize, or produced no disk image.
  const staple = spawn("node", ["scripts/desktop/notarize-disk-image.mjs"], {
    env: verificationEnvironment,
    stdio: "inherit",
  })
  if (staple.error) throw staple.error
  if (staple.status !== 0) return staple.status ?? 1

  const verify = spawn("node", ["scripts/desktop/verify-bundle.mjs"], {
    env: verificationEnvironment,
    stdio: "inherit",
  })
  if (verify.error) throw verify.error
  return verify.status ?? 1
}
