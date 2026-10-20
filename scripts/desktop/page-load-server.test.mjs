import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

test("native load HTTP control handles fragmented and refused requests within its bounds", () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-page-load-http-"))
  try {
    const binary = join(
      directory,
      process.platform === "win32" ? "server-tests.exe" : "server-tests",
    )
    const compiled = spawnSync(
      "rustc",
      [
        "--edition=2021",
        "--test",
        fileURLToPath(
          new URL(
            "../../src-tauri/examples/page_load_acceptance/server.rs",
            import.meta.url,
          ),
        ),
        "-D",
        "warnings",
        "-o",
        binary,
      ],
      { encoding: "utf8", timeout: 20_000 },
    )
    assert.equal(compiled.status, 0, compiled.stderr || compiled.error?.message)
    const run = spawnSync(binary, [], { encoding: "utf8", timeout: 8_000 })
    assert.equal(run.status, 0, run.stdout + run.stderr || run.error?.message)
    assert.match(run.stdout, /test result: ok\. 7 passed; 0 failed;/)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
