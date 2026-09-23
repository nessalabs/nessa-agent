import { afterEach, expect, it } from "vitest"
import { chmod, mkdir, mkdtemp, realpath, rm, symlink, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { LocalFileCredentialSource } from "./local-credential-source.js"

const roots: string[] = []
async function canonicalTemp(prefix: string): Promise<string> {
  return realpath(await mkdtemp(prefix))
}
afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  )
})
async function fixture() {
  const root = await canonicalTemp(join(tmpdir(), "nessa-source-"))
  roots.push(root)
  const folder = join(root, "ci/instances/one/auth/surfaces")
  await mkdir(folder, { recursive: true, mode: 0o700 })
  const file = join(folder, "chat.token")
  await writeFile(file, "fixture-secret\n", { mode: 0o600 })
  return {
    root,
    file,
    source: new LocalFileCredentialSource({
      dataDir: root,
      instance: "one",
      uid: process.getuid?.(),
    }),
  }
}
const context = {
  clientId: "chat",
  stage: "ci" as const,
  url: "ws://127.0.0.1:7420/session",
}
it("loads only the assigned surface in the matching stage and instance", async () => {
  const { source } = await fixture()
  expect(await source.load(context)).toBe("fixture-secret")
  await expect(source.load({ ...context, clientId: "another" })).rejects.toThrow()
  await expect(source.load({ ...context, stage: "prod" })).rejects.toThrow()
})
it("refuses to send automatically loaded credentials to remote gateways", async () => {
  const { source } = await fixture()
  await expect(
    source.load({ ...context, url: "wss://example.com/session" }),
  ).rejects.toThrow("loopback")
})
it("rejects public files, symlinks, and namespace traversal", async () => {
  const { source, file } = await fixture()
  await chmod(file, 0o644)
  await expect(source.load(context)).rejects.toThrow()
  await rm(file)
  await symlink("/etc/passwd", file)
  await expect(source.load(context)).rejects.toThrow()
  await expect(source.load({ ...context, clientId: "../owner" })).rejects.toThrow(
    "namespace",
  )
})

it("rejects a credential reached through a symlinked namespace", async () => {
  const root = await canonicalTemp(join(tmpdir(), "nessa-source-link-"))
  const outside = await canonicalTemp(join(tmpdir(), "nessa-source-outside-"))
  roots.push(root, outside)
  await chmod(root, 0o700)
  await chmod(outside, 0o700)
  const folder = join(outside, "auth/surfaces")
  await mkdir(folder, { recursive: true, mode: 0o700 })
  await writeFile(join(folder, "chat.token"), "redirected-secret\n", { mode: 0o600 })
  await symlink(outside, join(root, "ci"))
  const source = new LocalFileCredentialSource({
    dataDir: root,
    uid: process.getuid?.(),
  })
  await expect(source.load(context)).rejects.toThrow()
})

it("refuses a multi-hop root whose hidden hop is attacker-writable", async () => {
  const safe = await canonicalTemp(join(tmpdir(), "nessa-source-multihop-safe-"))
  const target = await canonicalTemp(join(tmpdir(), "nessa-source-multihop-target-"))
  roots.push(safe, target)
  const writable = join(target, "writable")
  const trusted = join(target, "trusted-target")
  const folder = join(trusted, "private/ci/instances/one/auth/surfaces")
  await mkdir(writable, { mode: 0o700 })
  await mkdir(folder, { recursive: true, mode: 0o700 })
  await writeFile(join(folder, "chat.token"), "redirected-secret\n", { mode: 0o600 })
  await symlink(trusted, join(writable, "hop"))
  await symlink(join(writable, "hop"), join(safe, "selected"))
  await chmod(writable, 0o777)

  const source = new LocalFileCredentialSource({
    dataDir: join(safe, "selected", "private"),
    instance: "one",
    uid: process.getuid?.(),
  })
  await expect(source.load(context)).rejects.toThrow()
})

it("never resolves localhost for automatic credential loading", async () => {
  const { source } = await fixture()
  for (const url of ["ws://localhost:7420/session", "wss://localhost:7420/session"]) {
    await expect(source.load({ ...context, url })).rejects.toThrow("loopback")
  }
})
