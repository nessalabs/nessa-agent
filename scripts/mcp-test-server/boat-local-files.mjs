/** Read-only local-file fixture for the Boat E2E check. Its only capability is
 * reading error.log in the explicitly supplied test root. No path comes from a
 * VM command, and this server has no write, execution or directory-list tool. */
import { constants } from "node:fs"
import { open } from "node:fs/promises"
import { createInterface } from "node:readline"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { PROTOCOL_VERSION } from "./server.mjs"

export const tool = {
  name: "read_local_log",
  description: "Read the explicitly shared error.log on the local gateway machine.",
  inputSchema: {
    type: "object",
    properties: { name: { type: "string", enum: ["error.log"] } },
    required: ["name"],
    additionalProperties: false,
  },
  annotations: { readOnlyHint: true, openWorldHint: false },
}

/** The real read is behind the supplied root; malformed paths, links and special
 * files fail before contents are read. The 4 KiB bound keeps MCP replies small. */
export async function answer(root, message) {
  if (message.id === undefined) return null
  const result = (value) => ({ jsonrpc: "2.0", id: message.id, result: value })
  switch (message.method) {
    case "initialize":
      return result({
        protocolVersion: PROTOCOL_VERSION,
        capabilities: { tools: {} },
        serverInfo: { name: "boat-local-files-fixture", version: "fixture" },
      })
    case "ping":
      return result({})
    case "tools/list":
      return result({ tools: [tool] })
    case "tools/call": {
      const args = message.params?.arguments
      if (
        message.params?.name !== tool.name ||
        !args ||
        Object.keys(args).length !== 1 ||
        !Object.hasOwn(args, "name") ||
        args.name !== "error.log"
      )
        return result({
          isError: true,
          content: [{ type: "text", text: "file_not_granted" }],
        })
      let file
      try {
        file = await open(
          join(root, "error.log"),
          constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
        )
        if (!(await file.stat()).isFile()) throw new Error("special file")
        const bytes = Buffer.alloc(4096)
        const { bytesRead } = await file.read(bytes, 0, bytes.length, 0)
        const text = bytes.subarray(0, bytesRead).toString("utf8")
        return result({
          content: [{ type: "text", text }],
          structuredContent: { text, host: process.platform },
        })
      } catch {
        return result({
          isError: true,
          content: [{ type: "text", text: "file_unavailable" }],
        })
      } finally {
        await file?.close()
      }
    }
    default:
      return {
        jsonrpc: "2.0",
        id: message.id,
        error: { code: -32601, message: "Method not found" },
      }
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = process.argv[2]
  if (!root) throw new Error("a fixture root is required")
  for await (const line of createInterface({
    input: process.stdin,
    crlfDelay: Infinity,
  })) {
    if (Buffer.byteLength(line) > 65536) break
    const reply = await answer(root, JSON.parse(line))
    if (reply) process.stdout.write(JSON.stringify(reply) + "\n")
  }
}
