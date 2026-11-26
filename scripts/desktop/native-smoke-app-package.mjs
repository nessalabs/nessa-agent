import assert from "node:assert/strict"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { builtExecutables } from "./native-smoke-build.mjs"
import {
  OwnedCommandCleanupError,
  runOwnedCommand,
  watchInterruptions,
} from "./native-smoke-processes.mjs"

export async function runAppPackageSmoke(root) {
  assert.equal(process.platform, "linux", "package native smoke requires Linux")
  assert.ok(process.env.DISPLAY, "run package native smoke with xvfb-run")
  const directory = mkdtempSync(join(tmpdir(), "nessa-native-package-"))
  const interruption = watchInterruptions()
  const { signal } = interruption
  let failure
  try {
    await runOwnedCommand("pkg-config", ["--exists", "webkit2gtk-4.1", "gtk+-3.0"], {
      cwd: root,
      signal,
      timeout: 10_000,
    })
    const output = await runOwnedCommand(
      "cargo",
      [
        "build",
        "--locked",
        "-p",
        "nessa-app",
        "--no-default-features",
        "--example",
        "app_package_acceptance",
        "--message-format=json-render-diagnostics",
      ],
      {
        cwd: root,
        signal,
        timeout: 12 * 60_000,
        captureLimit: 64 * 1024 * 1024,
      },
    )
    const executable = builtExecutables(output).get("app_package_acceptance")
    assert.ok(executable, "Cargo did not report the package acceptance executable")
    await runOwnedCommand(executable, [directory], {
      cwd: root,
      signal,
      timeout: 60_000,
      env: {
        ...process.env,
        HOME: directory,
        XDG_CONFIG_HOME: join(directory, "config"),
        XDG_DATA_HOME: join(directory, "data"),
        XDG_CACHE_HOME: join(directory, "cache"),
        WEBKIT_DISABLE_DMABUF_RENDERER: "1",
        WEBKIT_DISABLE_COMPOSITING_MODE: "1",
      },
    })
  } catch (error) {
    failure = error
  } finally {
    try {
      if (!(failure instanceof OwnedCommandCleanupError))
        rmSync(directory, { recursive: true, force: true })
    } catch (cleanup) {
      failure = failure ? new AggregateError([failure, cleanup]) : cleanup
    } finally {
      interruption.dispose()
    }
  }
  if (failure) throw failure
  console.log("native app package smoke passed and cleaned up")
}
