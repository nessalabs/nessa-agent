import { windowsPrivateFile } from "./windows-private-file.js"
import {
  NessaCredentialUnavailableError,
  type CredentialSource,
} from "../application/credential-source.js"
import { isLoopbackWebSocketUrl } from "../application/resolve-options.js"

/** Node-only private-file adapter. Native/browser hosts inject their own source. */
export class LocalFileCredentialSource implements CredentialSource {
  constructor(
    private readonly options: {
      dataDir?: string
      home?: string
      instance?: string
      uid?: number
      file?: string
    },
  ) {}

  async load(context: Parameters<CredentialSource["load"]>[0]): Promise<string> {
    if (!isLoopbackWebSocketUrl(context.url))
      throw new NessaCredentialUnavailableError(
        "Automatic local credentials may only be sent to a loopback gateway",
      )
    // Computed imports keep Node filesystem modules out of browser bundles.
    const fsModule = "node:fs/promises",
      pathModule = "node:path",
      constantsModule = "node:fs"
    const fs = (await import(
      /* @vite-ignore */ fsModule
    )) as typeof import("node:fs/promises")
    const path = (await import(
      /* @vite-ignore */ pathModule
    )) as typeof import("node:path")
    const { constants } = (await import(
      /* @vite-ignore */ constantsModule
    )) as typeof import("node:fs")
    const segment = (value: string) => /^[A-Za-z0-9_-]+$/.test(value)
    if (
      !segment(context.clientId) ||
      !segment(context.stage) ||
      (this.options.instance !== undefined && !segment(this.options.instance))
    )
      throw new NessaCredentialUnavailableError("Invalid local credential namespace")
    const base =
      this.options.dataDir ??
      (this.options.home ? path.join(this.options.home, ".nessa") : undefined)
    if (!base || !path.isAbsolute(base))
      throw new NessaCredentialUnavailableError(
        "An absolute local data directory is required",
      )
    let root = context.stage === "prod" ? base : path.join(base, context.stage)
    if (this.options.instance) root = path.join(root, "instances", this.options.instance)
    const file =
      this.options.file ??
      path.join(root, "auth", "surfaces", `${context.clientId}.token`)
    if (
      !path.isAbsolute(file) ||
      (process.platform !== "win32" && this.options.uid === undefined)
    )
      throw new NessaCredentialUnavailableError(
        "Private local file loading requires an absolute path and a supported OS owner check",
      )
    try {
      if (process.platform === "win32") {
        const credential = (await windowsPrivateFile("read", file)).trim()
        if (!credential || new TextEncoder().encode(credential).length > 16384)
          throw new Error("Invalid evidence")
        return credential
      }
      const handle = await fs.open(file, constants.O_RDONLY | constants.O_NOFOLLOW)
      try {
        const stat = await handle.stat()
        if (
          !stat.isFile() ||
          stat.nlink !== 1 ||
          stat.uid !== this.options.uid ||
          (stat.mode & 0o077) !== 0 ||
          stat.size < 1 ||
          stat.size > 16385
        )
          throw new Error("Invalid private credential file")
        const credential = (await handle.readFile("utf8")).trim()
        if (!credential || new TextEncoder().encode(credential).length > 16384)
          throw new Error("Invalid evidence")
        return credential
      } finally {
        await handle.close()
      }
    } catch {
      throw new NessaCredentialUnavailableError()
    }
  }
}
