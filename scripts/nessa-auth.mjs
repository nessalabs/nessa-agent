#!/usr/bin/env node
import { windowsPrivateFile } from "../packages/nessa-client/src/transport/windows-private-file.ts"
import { constants } from "node:fs"
import { open, readFile, unlink } from "node:fs/promises"
import { randomUUID } from "node:crypto"
import process from "node:process"
import { dirname } from "node:path"
import { WebSocket } from "ws"

globalThis.WebSocket ??= WebSocket

function usage() {
  return `Usage:
  pnpm auth:cli list --url ws://127.0.0.1:7420 --credential-file PATH
  pnpm auth:cli issue --url URL --credential-file PATH --input PATH --out PATH
  pnpm auth:cli revoke ID --url URL --credential-file PATH [--request-id ID]`
}

function argumentsFor(argv) {
  const [command, subject, ...rest] = argv
  const values = new Map()
  const words = command === "revoke" ? rest : [subject, ...rest].filter(Boolean)
  for (let index = 0; index < words.length; index += 2) {
    const flag = words[index]
    const value = words[index + 1]
    if (!flag?.startsWith("--") || !value) throw new Error(usage())
    values.set(flag.slice(2), value)
  }
  return { command, subject: command === "revoke" ? subject : undefined, values }
}

function required(values, name) {
  const value = values.get(name)
  if (!value) throw new Error(`--${name} is required\n${usage()}`)
  return value
}

async function readCredential(path) {
  if (process.platform === "win32") {
    const token = (await windowsPrivateFile("read", path)).trim()
    if (!token || Buffer.byteLength(token) > 16384)
      throw new Error("Invalid credential file")
    return token
  }
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW)
  try {
    const stat = await handle.stat()
    const currentUid = process.getuid?.()
    if (!stat.isFile() || stat.nlink !== 1)
      throw new Error("credential path must be a regular file")
    if (currentUid !== undefined && stat.uid !== currentUid) {
      throw new Error("credential file must be owned by the current user")
    }
    if ((stat.mode & 0o077) !== 0) {
      throw new Error("credential file permissions must deny group and other access")
    }
    if (stat.size < 1 || stat.size > 16 * 1024) {
      throw new Error("credential file must contain 1 to 16384 bytes")
    }
    const credential = (await handle.readFile("utf8")).trim()
    if (!credential) throw new Error("credential file is empty")
    return credential
  } finally {
    await handle.close()
  }
}

async function syncDirectory(path) {
  if (process.platform === "win32") return
  const directory = await open(dirname(path), constants.O_RDONLY)
  try {
    await directory.sync()
  } finally {
    await directory.close()
  }
}

async function readIssueInput(path) {
  try {
    const input = JSON.parse(await readFile(path, "utf8"))
    if (typeof input !== "object" || input === null || Array.isArray(input))
      throw new Error()
    if (
      input.requestId !== undefined &&
      (typeof input.requestId !== "string" || input.requestId.length === 0)
    ) {
      throw new Error()
    }
    return { ...input, requestId: input.requestId ?? randomUUID() }
  } catch {
    throw new Error("issue input must be valid JSON with an optional non-empty requestId")
  }
}

async function main() {
  const { NessaClient } = await import("@nessa/client")
  const { command, subject, values } = argumentsFor(process.argv.slice(2))
  if (!new Set(["issue", "list", "revoke"]).has(command)) throw new Error(usage())
  const secretOutput = command === "issue" ? required(values, "out") : undefined
  let secretFile
  if (secretOutput) {
    if (process.platform === "win32") {
      await windowsPrivateFile("reserve", secretOutput)
      secretFile = {
        writeFile: (value) => windowsPrivateFile("write", secretOutput, value),
        sync: async () => {},
        close: async () => {},
      }
    } else {
      secretFile = await open(
        secretOutput,
        constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW,
        0o600,
      )
    }
  }

  try {
    const credential = await readCredential(required(values, "credential-file"))
    const client = await NessaClient.connect({
      profile: "product",
      stage: "dev",
      url: required(values, "url"),
      role: "surface",
      surface: { kind: "cli", instance: "nessa-auth" },
      client: { id: "nessa-auth-cli", version: "0.1.0", platform: "node" },
      auth: { credential },
    })

    try {
      if (command === "list") {
        console.log(JSON.stringify(await client.credentials.list(), null, 2))
        return
      }
      if (command === "revoke") {
        if (!subject) throw new Error(`credential ID is required\n${usage()}`)
        console.log(
          JSON.stringify(
            await client.credentials.revoke(
              subject,
              values.get("request-id") ?? randomUUID(),
            ),
            null,
            2,
          ),
        )
        return
      }

      const input = await readIssueInput(required(values, "input"))
      console.error(`requestId: ${input.requestId}`)
      const result = await client.credentials.issue(input)
      if ("secret" in result) {
        try {
          await secretFile.writeFile(`${result.secret}\n`, "utf8")
          await secretFile.sync()
          await secretFile.close()
          secretFile = undefined
          await syncDirectory(secretOutput)
        } catch {
          console.error(
            `credential ${result.credential.id} was issued but its secret could not be stored; revoke it and reissue with a new requestId`,
          )
          throw new Error("issued credential secret was not delivered")
        }
      } else {
        await secretFile.close()
        secretFile = undefined
        await unlink(secretOutput)
        await syncDirectory(secretOutput)
      }
      console.log(
        JSON.stringify(
          "secret" in result
            ? { credential: result.credential, secretWrittenTo: secretOutput }
            : result,
          null,
          2,
        ),
      )
    } finally {
      client.close()
    }
  } finally {
    if (secretFile) {
      await secretFile.close().catch(() => {})
      await unlink(secretOutput).catch(() => {})
    }
  }
}

main().catch((error) => {
  // SDK and server errors are redacted; never print command inputs or credential evidence.
  console.error(error instanceof Error ? error.message : "credential command failed")
  process.exitCode = 1
})
