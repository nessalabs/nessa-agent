/** Windows runtime checks for the Node Win32 bridge; no credential leaves this fixture. */
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { link, mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { windowsPrivateFile } from "../packages/nessa-client/src/transport/windows-private-file.js"
import { LocalFileCredentialSource } from "../packages/nessa-client/src/transport/local-credential-source.js"

assert.equal(process.platform, "win32")
const root = await mkdtemp(join(tmpdir(), "nessa-acl-"))
const file = join(root, "token")
try {
  await windowsPrivateFile("reserve", file)
  await windowsPrivateFile("write", file, "fixture-only\n")
  assert.equal(await windowsPrivateFile("read", file), "fixture-only\n")
  await assert.rejects(windowsPrivateFile("reserve", file))
  const source = new LocalFileCredentialSource({ dataDir: root, file })
  assert.equal(
    await source.load({
      clientId: "chat",
      stage: "ci",
      url: "ws://127.0.0.1:7420/session",
    }),
    "fixture-only",
  )
  await assert.rejects(
    source.load({ clientId: "chat", stage: "ci", url: "wss://example.com/session" }),
  )
  const alias = join(root, "alias")
  await link(file, alias)
  await assert.rejects(windowsPrivateFile("read", file))
  await rm(alias)
  const change = spawnSync("icacls.exe", [file, "/grant", "*S-1-1-0:R"], {
    encoding: "utf8",
  })
  assert.equal(change.status, 0, "could not make broad ACL fixture")
  await assert.rejects(windowsPrivateFile("read", file))
  const inherited = join(root, "inherited")
  await writeFile(inherited, "fixture")
  await assert.rejects(windowsPrivateFile("read", inherited))
  console.log(
    "Windows Node ACL creation, loading, hard-link and broad/inherited ACL rejection passed",
  )
} finally {
  await rm(root, { recursive: true, force: true })
}
