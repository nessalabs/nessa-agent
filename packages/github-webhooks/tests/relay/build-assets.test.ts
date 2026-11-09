import assert from "node:assert/strict"
import { test } from "node:test"
import { mkdtemp, mkdir, readFile, writeFile, copyFile, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { promisify } from "node:util"
import { execFile } from "node:child_process"

test("Cedar assets build in checkout paths containing spaces", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "nessa relay build "))
  t.after(() => rm(directory, { recursive: true, force: true }))
  await mkdir(join(directory, "scripts"))
  const dependency = join(directory, "node_modules/@cedar-policy/cedar-wasm/web")
  await mkdir(dependency, { recursive: true })
  await writeFile(
    join(dependency, "../package.json"),
    JSON.stringify({
      type: "module",
      exports: { "./web": { import: "./web/cedar_wasm.js" } },
    }),
  )
  await writeFile(
    join(dependency, "cedar_wasm.js"),
    "// Resolution fixture; not executed.\n",
  )
  await writeFile(join(dependency, "cedar_wasm_bg.wasm"), "fixture bytes")
  await copyFile(
    new URL("../../scripts/prepare-cedar.mjs", import.meta.url),
    join(directory, "scripts/prepare-cedar.mjs"),
  )
  await promisify(execFile)(
    process.execPath,
    [join(directory, "scripts/prepare-cedar.mjs")],
    { cwd: directory },
  )
  assert.equal(
    await readFile(
      join(directory, "src/relay/infrastructure/cedar/generated/cedar.wasm"),
      "utf8",
    ),
    "fixture bytes",
  )
})
