// The public framenavigated event also includes same-document hash changes.
// Observe Chromium's new-document loader and DOM lifecycle through the existing
// private Playwright pipe; no debugging listener is opened.
export async function navigatePage(page, url) {
  const observer = await page.context().newCDPSession(page)
  let timer, onFrame, onLifecycle, outcome
  try {
    await observer.send("Page.enable")
    await observer.send("Page.setLifecycleEventsEnabled", { enabled: true })
    const {
      frameTree: { frame: initial },
    } = await observer.send("Page.getFrameTree")
    const isNewLoader = (id) => Boolean(id && id !== initial.loaderId)
    let loader
    const readyLoaders = new Set()
    const settled = new Promise((resolve) => {
      const complete = () => {
        if (loader && readyLoaders.has(loader)) resolve({ ok: true })
      }
      onFrame = ({ frame }) => {
        if (frame.id !== initial.id || !isNewLoader(frame.loaderId)) return
        if (frame.url !== url && frame.unreachableUrl !== url) return
        loader = frame.loaderId
        complete()
      }
      onLifecycle = (event) => {
        if (
          event.frameId !== initial.id ||
          event.name !== "DOMContentLoaded" ||
          !isNewLoader(event.loaderId)
        )
          return
        readyLoaders.add(event.loaderId)
        complete()
      }
      observer.on("Page.frameNavigated", onFrame)
      observer.on("Page.lifecycleEvent", onLifecycle)
      timer = setTimeout(
        () => resolve({ ok: false, error: new Error("navigation_settlement_timeout") }),
        5000,
      )
    })
    try {
      await page.goto(url, { waitUntil: "domcontentloaded", timeout: 5000 })
      outcome = { kind: "complete" }
    } catch (cause) {
      const settlement = await settled
      outcome = settlement.ok
        ? { kind: "failed", cause }
        : { kind: "unsettled", cause, settlementError: settlement.error }
    }
  } catch (cause) {
    outcome = { kind: "unsettled", cause, settlementError: cause }
  } finally {
    clearTimeout(timer)
    if (onFrame) observer.off("Page.frameNavigated", onFrame)
    if (onLifecycle) observer.off("Page.lifecycleEvent", onLifecycle)
    try {
      await observer.detach()
    } catch (settlementError) {
      outcome = {
        kind: "unsettled",
        cause: outcome?.cause ?? settlementError,
        settlementError,
      }
    }
  }
  return outcome
}
