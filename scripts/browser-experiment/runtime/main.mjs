import { fileURLToPath } from "node:url"
import { fixtureSite } from "../fixtures/site.mjs"
import { launchBrowser } from "./browser.mjs"
import { viewerServer } from "./server.mjs"
import { startExperiment } from "./experiment.mjs"

// Composition accepts only the experiment's named test profiles, not an existing browser path.
const profileName = process.env.EXPERIMENT_PROFILE ?? "a"
if (!["a", "b"].includes(profileName))
  throw new Error("EXPERIMENT_PROFILE must be a or b")
const fixture = await fixtureSite(17891)
let experiment
let cleanupOwner
try {
  experiment = await startExperiment({
    fixture,
    openBrowser: (home) =>
      launchBrowser({
        profile: fileURLToPath(
          new URL(`../.local/profiles/${profileName}`, import.meta.url),
        ),
        downloads: fileURLToPath(
          new URL(`../.local/downloads/${profileName}`, import.meta.url),
        ),
        home,
        executablePath: process.env.EXPERIMENT_CHROMIUM,
      }),
    openViewer: (browser) => viewerServer(browser.session, browser.savedDownloads),
  })
  // Contains a capability: print only to the local operator, never commit this output.
  console.log(JSON.stringify({ viewer: experiment.viewer.url, profile: profileName }))
} catch (error) {
  cleanupOwner = error.cleanupOwner
  console.error("Browser experiment could not start. Sandbox remains enabled.", error)
  process.exitCode = 1
}
async function close() {
  try {
    await (experiment ?? cleanupOwner)?.close()
  } catch (error) {
    console.error("Browser experiment cleanup failed", error)
    process.exitCode = 1
  }
}
process.on("SIGINT", () => void close())
process.on("SIGTERM", () => void close())
