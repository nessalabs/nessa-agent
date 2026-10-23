import { closeResources, failAfterCleanup } from "./cleanup.mjs"

// The fixture is already owned when startup begins. Preserve the primary failure
// and attempt the remaining cleanup even when one close fails.
export async function startExperiment({ fixture, openBrowser, openViewer }) {
  let browser
  let viewer
  try {
    browser = await openBrowser(fixture.home)
    viewer = await openViewer(browser)
  } catch (cause) {
    await failAfterCleanup("startup_failed", cause, [viewer, browser, fixture])
  }
  let closing
  return {
    browser,
    viewer,
    close: () => (closing ??= closeResources([viewer, browser, fixture])),
  }
}
