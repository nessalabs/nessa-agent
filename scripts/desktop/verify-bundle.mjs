import { execFileSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { resolve } from "node:path"
import { bundleArchitecture, includesDiskImage } from "./bundle-architecture.mjs"
import { verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"

const root = resolve(import.meta.dirname, "../..")
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: root,
    encoding: "utf8",
  }),
)
const target = process.env.NESSA_BUILD_TARGET
const release = target
  ? resolve(metadata.target_directory, target, "release")
  : resolve(metadata.target_directory, "release")
const app = resolve(release, "bundle/macos/Nessa.app")
if (!existsSync(app)) throw new Error(`macOS application bundle is absent: ${app}`)
const config = JSON.parse(readFileSync(resolve(root, "src-tauri/tauri.conf.json")))

const runtime = resolve(app, "Contents/Resources/runtime")
verifyRuntimeFingerprint(runtime)

execFileSync("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app], {
  stdio: "inherit",
})
if (
  process.env.APPLE_SIGNING_IDENTITY?.trim() &&
  process.env.APPLE_SIGNING_IDENTITY.trim() !== "-"
)
  execFileSync("spctl", ["--assess", "--type", "execute", "--verbose=2", app], {
    stdio: "inherit",
  })

if (includesDiskImage(process.env.NESSA_BUILD_BUNDLES, config.bundle.targets)) {
  const architecture = bundleArchitecture(target, process.arch)
  const dmg = resolve(
    release,
    `bundle/dmg/${config.productName}_${config.version}_${architecture}.dmg`,
  )
  if (!existsSync(dmg)) throw new Error(`macOS disk image is absent: ${dmg}`)
  execFileSync("hdiutil", ["verify", dmg], { stdio: "inherit" })
  if (
    process.env.APPLE_SIGNING_IDENTITY?.trim() &&
    process.env.APPLE_SIGNING_IDENTITY.trim() !== "-"
  )
    execFileSync("codesign", ["--verify", "--strict", "--verbose=2", dmg], {
      stdio: "inherit",
    })
}
