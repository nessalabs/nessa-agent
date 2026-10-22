// Toggles the sidebar with ⌘B and samples where the columns are on each frame.
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
const sample = () =>
  page.evaluate(
    () =>
      new Promise((done) => {
        const rows = []
        const t0 = performance.now()
        const pick = (sel) => {
          const e = document.querySelector(sel)
          if (!e) return null
          const r = e.getBoundingClientRect()
          return Math.round(r.left)
        }
        const step = () => {
          rows.push({
            t: Math.round(performance.now() - t0),
            sidebar: pick(".workspace-sidebar"),
            list: pick(".workspace-list"),
            pane: pick("article.workspace-pane"),
            tabs: pick(".workspace-window-tabs"),
          })
          if (performance.now() - t0 < 700) requestAnimationFrame(step)
          else done(rows)
        }
        requestAnimationFrame(step)
      }),
  )
for (const dir of ["close", "open"]) {
  const p = sample()
  await page.keyboard.press("Meta+b")
  const rows = await p
  const moving = rows.filter((r, i) => i > 0 && JSON.stringify({ ...r, t: 0 }) !== JSON.stringify({ ...rows[i - 1], t: 0 }))
  console.log(dir, "frames that moved:", moving.length, "last move at", moving.at(-1)?.t, "ms")
  console.log(rows.filter((_, i) => i % 3 === 0).map((r) => `${r.t}:${r.sidebar}/${r.list}/${r.pane}/${r.tabs}`).join("  "))
  await page.waitForTimeout(500)
}
await browser.close()
