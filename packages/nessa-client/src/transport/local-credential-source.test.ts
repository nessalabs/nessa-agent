import { afterEach, expect, it } from "vitest"
import { chmod, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { LocalFileCredentialSource } from "./local-credential-source.js"

const roots: string[] = []
afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  )
})
async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "nessa-source-"))
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
