import assert from "node:assert/strict"
import { test } from "node:test"
import { mkdtemp, mkdir, writeFile, symlink, rm } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { answer } from "./boat-local-files.mjs"

const request = (name) => ({
  id: 1,
  method: "tools/call",
  params: { name: "read_local_log", arguments: { name } },
})
test("the shared local log is read, while outside paths and replacement links are refused", async () => {
  const root = await mkdtemp(join(tmpdir(), "nessa-boat-local-fixture-"))
  const allowed = join(root, "allowed")
  try {
    await mkdir(allowed)
    await writeFile(join(allowed, "error.log"), "LOCAL_LOG_MARKER")
    await writeFile(join(root, "outside.log"), "NOT_GRANTED")
    assert.equal(
      (await answer(allowed, request("error.log"))).result.structuredContent.text,
      "LOCAL_LOG_MARKER",
    )
    for (const name of ["../outside.log", join(root, "outside.log"), "other.log"])
      assert.equal(
        (await answer(allowed, request(name))).result.content[0].text,
        "file_not_granted",
      )
    await rm(join(allowed, "error.log"))
    await symlink(join(root, "outside.log"), join(allowed, "error.log"))
    assert.equal(
      (await answer(allowed, request("error.log"))).result.content[0].text,
      "file_unavailable",
    )
    assert.equal(
      (await answer(allowed, { id: 2, method: "tools/list" })).result.tools.length,
      1,
    )
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
