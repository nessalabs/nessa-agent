/**
 * The parts of a release manifest a local harness has to get exactly right.
 *
 * Kept apart from `updater-harness.mjs` — which signs, serves, and prints — so
 * the details the plugin is fussy about can be tested without a key, a server,
 * or a built artifact. Every one of them is a silent failure when wrong: the
 * app reports no update rather than an error.
 */

/** The `{os}-{arch}` key `tauri-plugin-updater` looks up in `platforms`.
 *
 * The plugin's own `updater_os`/`updater_arch` decide this, and they do not
 * spell things the way Node does: macOS is `darwin`, not `macos`, and the
 * architectures are Rust's. A key that looks right but is not makes the
 * manifest simply not apply, which reads as "already up to date". */
export function updaterTarget(platform, arch) {
  const os = { darwin: "darwin", linux: "linux", win32: "windows" }[platform]
  const cpu = {
    x64: "x86_64",
    arm64: "aarch64",
    ia32: "i686",
    arm: "armv7",
    riscv64: "riscv64",
  }[arch]
  if (!os) throw new Error(`No updater target for platform ${platform}`)
  if (!cpu) throw new Error(`No updater target for architecture ${arch}`)
  return `${os}-${cpu}`
}

/** The release manifest, in the shape the plugin's `RemoteRelease` reads.
 *
 * `version`, `notes` and `pub_date` describe the release; `platforms.<target>`
 * is what this machine downloads and the signature it is checked against.
 * `pub_date` must be RFC 3339 or the plugin rejects the whole manifest while
 * parsing it, before it ever looks at the platform. */
export function releaseManifest({ version, notes, target, signature, url, published }) {
  return {
    version,
    notes,
    pub_date: published,
    platforms: { [target]: { signature, url } },
  }
}

/** Where `createUpdaterArtifacts` leaves the thing an update installs.
 *
 * One per platform, because the plugin installs a different kind of thing on
 * each: an archived app bundle on macOS, an archived AppImage on Linux, the
 * NSIS installer on Windows. */
export function defaultArtifacts(platform, version) {
  const artifacts = {
    darwin: ["macos/Nessa.app.tar.gz"],
    linux: [`appimage/Nessa_${version}_amd64.AppImage.tar.gz`],
    win32: [`nsis/Nessa_${version}_x64-setup.exe`],
  }[platform]
  if (!artifacts)
    throw new Error(`No updater artifact is bundled for platform ${platform}`)
  return artifacts.map((artifact) => `target/release/bundle/${artifact}`)
}

/** Read one `--name value` or `--name=value` argument. */
export function option(args, name, fallback) {
  const index = args.indexOf(`--${name}`)
  if (index >= 0) {
    const value = args[index + 1]
    if (!value || value.startsWith("--")) throw new Error(`--${name} requires a value`)
    return value
  }
  const equals = args.find((argument) => argument.startsWith(`--${name}=`))
  return equals ? equals.slice(name.length + 3) : fallback
}
