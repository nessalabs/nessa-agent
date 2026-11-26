import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { once } from "node:events"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { builtExecutables } from "./native-smoke-build.mjs"
import { stopOwnedGroup, watchInterruptions } from "./native-smoke-processes.mjs"

export async function runAppPackageSmoke(root) {
  assert.equal(process.platform, "linux", "package native smoke requires Linux")
  assert.ok(process.env.DISPLAY, "run package native smoke with xvfb-run")
  const native = spawnSync("pkg-config", ["--exists", "webkit2gtk-4.1", "gtk+-3.0"])
  if (native.error) throw native.error
  assert.equal(native.status, 0, "package native smoke requires WebKitGTK and GTK")

  const directory = mkdtempSync(join(tmpdir(), "nessa-native-package-"))
  const interruption = watchInterruptions()
  let child
  let failure
  try {
    const build = spawnSync(
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
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
        stdio: ["inherit", "pipe", "inherit"],
      },
    )
    if (build.error) throw build.error
    assert.equal(build.status, 0, "package native smoke executable failed to build")
    const executable = builtExecutables(build.stdout).get("app_package_acceptance")
    assert.ok(executable, "Cargo did not report the package acceptance executable")
    interruption.signal.throwIfAborted()
    child = spawn(executable, [directory], {
      cwd: root,
      detached: true,
      stdio: "inherit",
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
    const [code, signal] = await once(child, "exit", {
      signal: AbortSignal.any([interruption.signal, AbortSignal.timeout(60_000)]),
    })
    assert.equal(code, 0, `package native smoke exited with ${signal ?? code}`)
  } catch (error) {
    failure = error
  } finally {
    try {
      assert.ok(await stopOwnedGroup(child?.pid), "package process cleanup unconfirmed")
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
