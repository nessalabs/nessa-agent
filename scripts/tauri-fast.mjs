/**
 * The testing-shaped release: same binary as `app:build`, without the slow
 * installer. `tauri build --bundles` is OS-specific — macOS has `app`/`dmg`,
 * Linux has `deb`/`rpm`/`appimage`, Windows has `nsis`/`msi` — so hardcoding
 * `--bundles app` in package.json is what made `pnpm app:fast` fail on Linux.
 *
 * The analog of "the .app, no dmg" is:
 *   macOS   → app
 *   Linux   → deb
 *   Windows → nsis
 */
import { spawnSync } from "node:child_process"

const bundle = {
  darwin: "app",
  linux: "deb",
  win32: "nsis",
}[process.platform]

if (!bundle) {
  console.error(`app:fast: no bundle for ${process.platform}`)
  process.exit(1)
}

const result = spawnSync("pnpm", ["exec", "tauri", "build", "--bundles", bundle], {
  stdio: "inherit",
  env: process.env,
})
process.exit(result.status ?? 1)
