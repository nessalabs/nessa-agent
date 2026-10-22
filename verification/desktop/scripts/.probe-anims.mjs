import { chromium } from "playwright"
const url = process.argv[2]
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
await page.addInitScript(() => {
  localStorage.setItem("nessa.desktop.workspace-layout", "columns")
  Object.defineProperty(navigator, "platform", { get: () => "MacIntel" })
})
await page.goto(url)
await page.waitForSelector("article.workspace-pane", { timeout: 20000 })
await page.waitForTimeout(1500)
for (const dir of ["close", "open"]) {
  await page.keyboard.press("Meta+b")
  const seen = await page.evaluate(() => new Promise((done) => {
    const all = new Map()
    const t0 = performance.now()
    const look = () => {
      for (const a of document.getAnimations()) {
        const el = a.effect?.target
        const name = el ? `${el.tagName.toLowerCase()}.${String(el.className).split(" ")[0]}` : "?"
        const t = a.effect.getTiming()
        const props = a instanceof CSSTransition ? a.transitionProperty : (a.animationName || (a.effect.getKeyframes?.()[0] ? Object.keys(a.effect.getKeyframes()[0]).filter(k=>!["offset","easing","composite","computedOffset"].includes(k)).join(",") : ""))
        all.set(name + " " + props, `${Math.round(t.duration)}ms ${t.easing} delay${t.delay}`)
      }
      if (performance.now() - t0 < 600) requestAnimationFrame(look); else done([...all])
    }
    look()
  }))
  console.log(dir); for (const [k, v] of seen) console.log("  ", k, v)
  await page.waitForTimeout(600)
}
await browser.close()
