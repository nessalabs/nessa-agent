import { chromium } from "playwright"
import { mkdir, chmod } from "node:fs/promises"
import { fileURLToPath } from "node:url"
import { randomUUID } from "node:crypto"
import { join } from "node:path"
import { createSession, ExperimentFailure } from "./session.mjs"
import { failAfterCleanup } from "./cleanup.mjs"

export function launchOptions(executablePath) {
  const extension = fileURLToPath(new URL("../fixtures/extension", import.meta.url))
  return {
    headless: false,
    chromiumSandbox: true,
    channel: "chromium",
    ...(executablePath ? { executablePath } : {}),
    viewport: { width: 1000, height: 700 },
    acceptDownloads: true,
    timeout: 15000,
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`],
  }
}

export async function launchBrowser({ profile, downloads, home, executablePath }) {
  if (process.getuid?.() === 0) throw new ExperimentFailure("non_root_required")
  await mkdir(profile, { recursive: true, mode: 0o700 })
  await chmod(profile, 0o700)
  await mkdir(downloads, { recursive: true, mode: 0o700 })
  // Playwright uses a private debugging pipe, not a listening CDP TCP port.
  const context = await chromium.launchPersistentContext(
    profile,
    launchOptions(executablePath),
  )
  return initializeBrowser(context, { downloads, home })
}

export async function initializeBrowser(context, { downloads, home }) {
  try {
    const page = context.pages()[0] ?? (await context.newPage())
    const session = createSession(page, home)
    const savedDownloads = []
    const downloadTasks = new Set()
    page.on("download", (download) => {
      const id = randomUUID()
      const task = download.saveAs(join(downloads, id)).then(
        () => savedDownloads.push({ id, state: "saved" }),
        () => savedDownloads.push({ id, state: "failed" }),
      )
      downloadTasks.add(task)
      task.finally(() => downloadTasks.delete(task))
    })
    context.on("close", () => session.disconnected())
    await session.run({ type: "navigate", target: "home" })
    return {
      context,
      page,
      session,
      savedDownloads,
      close: async () => {
        await session.close()
        await context.close()
        await Promise.all(downloadTasks)
      },
    }
  } catch (error) {
    await failAfterCleanup("browser_initialization_failed", error, [context])
  }
}
