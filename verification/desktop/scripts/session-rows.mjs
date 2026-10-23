#!/usr/bin/env node
/**
 * What a session row does in place (`session-list.tsx`,
 * `session-row-actions.tsx`): Archive and "More" under the pointer, Rename
 * where the title was, Delete once asked, and a two-finger swipe left that
 * uncovers Archive and Delete as Mail's rows do, while a mouse drag stays the
 * drag to a pane and a right-click stays the row's menu.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  {
    name: "session-rows",
    summary: "a session row's hover buttons, More menu, rename, delete and swipe",
    defaults: { engine: "chromium" },
    help: `
Checks, per engine:
  hover     away from a row its time shows and its buttons do not; under the
            pointer Archive and More take the time's place, 26 px each
  more      More lists Rename, Pin, Archive and Delete…, and the row keeps its
            buttons while it is open; Rename edits the title in place and ↩
            keeps it
  delete    Delete… asks first; Cancel deletes nothing and the menu opens
            again after it; Delete removes the row
  archive   the Archive button archives the row and does not open it; Archive
            from the menu hands focus to the row in its place
  swipe     two fingers left uncover Archive and Delete without opening the
            row; Delete asks once more in place, then removes the row; a long
            swipe rests open and never archives, and Archive is one press; a
            mouse drag moves nothing and right-click is the row's own menu`,
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      const check = (name, body) =>
        attempt(rep, { name, engine }, async () => {
          const { page, close } = await openPage(browser, { url, layout: "columns" })
          try {
            await need(page, css.sessionRow, "a session row")
            return await body(page)
          } finally {
            await close().catch(() => {})
          }
        })

      await check("hover", async (page) => {
        const row = await quietRow(page)
        const failures = []
        const away = await parts(row)
        if (!away.time) failures.push("the time is hidden away from the row")
        if (away.buttons.length) failures.push("buttons show away from the row")
        await row.hover()
        const under = await parts(row)
        if (under.time) failures.push("the time still shows under the pointer")
        const labels = under.buttons.map((each) => each.action)
        if (labels.join() !== "archive,more") failures.push(`buttons: ${labels}`)
        for (const each of under.buttons)
          if (Math.round(each.width) !== 26 || Math.round(each.height) !== 26)
            failures.push(`${each.action} is ${each.width}x${each.height}`)
        const evidence = await shot(page, options, engine, "hover", row)
        return { failures, measured: { away, under }, evidence }
      })

      await check("more", async (page) => {
        const row = await quietRow(page)
        const id = await row.getAttribute("data-session-row")
        const failures = []
        await openMore(page, row)
        const words = await page.locator(css.menuItem).allInnerTexts()
        // Share… is there for a session the window may share.
        const asked = words.filter((word) => word !== "Share…").join("|")
        // A pinned row offers Unpin in Pin's place.
        if (!/^Rename\|(Pin|Unpin)\|Fork\|Archive\|Delete…$/.test(asked))
          failures.push(`the menu read ${JSON.stringify(words)}`)
        if ((await row.getAttribute("data-actions")) !== "true")
          failures.push("the row let its buttons go while its menu was open")
        const menuShot = await shot(page, options, engine, "more", row, 160)
        await page.getByRole("menuitem", { name: "Rename" }).click()
        const field = row.locator("[data-session-rename]")
        await field.waitFor({ timeout: 3_000 })
        if (!(await field.evaluate((input) => input === document.activeElement)))
          failures.push("the field did not take focus")
        await field.fill("Rain plan")
        const renameShot = await shot(page, options, engine, "rename", row)
        await page.keyboard.press("Enter")
        await field.waitFor({ state: "detached", timeout: 3_000 })
        // The title the source keeps, once it says so, and still after the
        // list has been said again.
        const label = rowById(page, id).locator(".desktop-list-row-label")
        const said = await waitText(label, "Rain plan")
        if (said !== "Rain plan") failures.push(`the title read ${JSON.stringify(said)}`)
        await page.waitForTimeout(1_000)
        const kept = await label.innerText()
        if (kept !== "Rain plan")
          failures.push(`the title went back to ${JSON.stringify(kept)}`)
        if ((await focusedRow(page)) !== id)
          failures.push("↩ did not give focus back to the row")

        // An item that asks nothing more hands focus to the row, not to the
        // pointer's button that opened the menu.
        await openMore(page, rowById(page, id))
        await page.evaluate(() => {
          window.__pinMenuFrames = []
          window.__pinMenuFramesDone = false
          const start = performance.now()
          const tick = () => {
            for (const menu of document.querySelectorAll('[role="menu"]')) {
              const box = menu.getBoundingClientRect()
              if (Number(getComputedStyle(menu).opacity) > 0)
                window.__pinMenuFrames.push({
                  x: box.x,
                  y: box.y,
                  state: menu.dataset.state,
                })
            }
            if (performance.now() - start < 400) requestAnimationFrame(tick)
            else window.__pinMenuFramesDone = true
          }
          requestAnimationFrame(tick)
        })
        await page.getByRole("menuitem", { name: /^(Pin|Unpin)$/ }).click()
        await page.waitForFunction(() => window.__pinMenuFramesDone)
        const pinFrames = await page.evaluate(() => window.__pinMenuFrames)
        if (pinFrames.some((frame) => frame.x < 16 && frame.y < 16))
          failures.push("Pin paints a detached menu at the window's top-left")
        await page.locator(css.menuItem).first().waitFor({ state: "detached" })
        if ((await focusedRow(page)) !== id)
          failures.push(`after Pin, focus was on ${await focusedRow(page)}, not the row`)
        return { failures, evidence: [menuShot, renameShot].filter(Boolean) }
      })

      await check("delete", async (page) => {
        const row = await quietRow(page)
        const id = await row.getAttribute("data-session-row")
        const failures = []
        const ask = async () => {
          await openMore(page, row)
          await page.getByRole("menuitem", { name: "Delete…" }).click()
          await need(page, "[data-delete-session]", "the delete question")
        }
        await ask()
        const evidence = await shot(page, options, engine, "delete", null)
        await page.getByRole("button", { name: "Cancel" }).click()
        await page.locator("[data-delete-session]").waitFor({ state: "detached" })
        if ((await rowById(page, id).count()) !== 1)
          failures.push("Cancel deleted the row")
        if ((await waitFocusedRow(page, (now) => now === id)) !== id)
          failures.push(
            `after Cancel, focus was on ${await focusedRow(page)}, not the row`,
          )
        // The menu opens again after the question, and stays open: still
        // there once its own opening has settled, past every frame of it.
        await openMore(page, row)
        const content = page.locator('[role="menu"]').first()
        await content.evaluate((element) =>
          Promise.all(element.getAnimations().map((each) => each.finished)),
        )
        await page.evaluate(
          () =>
            new Promise((resolve) =>
              requestAnimationFrame(() => requestAnimationFrame(resolve)),
            ),
        )
        if ((await page.locator(css.menuItem).count()) === 0)
          failures.push("the menu closed as it opened after the question")
        await page.keyboard.press("Escape")
        await ask()
        await page.locator('[data-delete-action="confirm"]').click()
        await rowById(page, id).waitFor({ state: "detached", timeout: 3_000 })
        const after = await waitFocusedRow(page, (now) => now !== null && now !== id)
        if (after === null || after === id)
          failures.push(`after Delete, focus was on ${after}, not the row beside it`)
        return { failures, evidence }
      })

      await check("archive", async (page) => {
        const row = await quietRow(page)
        const id = await row.getAttribute("data-session-row")
        const before = await openId(page)
        await row.hover()
        await row.locator('[data-session-action="archive"]').click()
        await rowById(page, id).waitFor({ state: "detached", timeout: 3_000 })
        const after = await openId(page)
        const failures = [after === before ? null : `the open session became ${after}`]

        // Archived from its menu, the row hands focus to the one now in its
        // place, in this list, so the keys go on from there.
        const next = await quietRow(page)
        const nextId = await next.getAttribute("data-session-row")
        await openMore(page, next)
        await page.getByRole("menuitem", { name: "Archive" }).click()
        await rowById(page, nextId).waitFor({ state: "detached", timeout: 3_000 })
        const focused = await waitFocusedRow(
          page,
          (now) => now !== null && now !== nextId,
        )
        if (focused === null || focused === nextId)
          failures.push(`after Archive from the menu, focus was on ${focused}, not a row`)
        return { failures: failures.filter(Boolean) }
      })

      await check("swipe", async (page) => {
        const failures = []
        const before = await openId(page)
        let row = await quietRow(page)
        let id = await row.getAttribute("data-session-row")
        row = rowById(page, id)
        const wrapper = () => rowById(page, id).locator("xpath=..")

        // A mouse drag is the drag to a pane: the row does not slide.
        const box = await row.boundingBox()
        await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
        await page.mouse.down()
        await page
          .locator(".split-panes-ghost[data-drag-waiting]")
          .waitFor({ state: "attached", timeout: 3_000 })
        await page.mouse.move(box.x + 20, box.y + box.height / 2, { steps: 8 })
        await page
          .locator(".split-panes-ghost:not([data-drag-waiting])")
          .waitFor({ state: "attached", timeout: 3_000 })
        const dragged = await wrapper().getAttribute("data-swipe")
        await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 4 })
        await page.mouse.up()
        if (dragged) failures.push(`a mouse drag slid the row (${dragged})`)
        await page.keyboard.press("Escape")

        // Right-click is the row's menu, not its swipe actions.
        await row.click({ button: "right" })
        await need(page, css.menuItem, "the row's menu")
        if (await wrapper().getAttribute("data-swipe"))
          failures.push("right-click uncovered the swipe actions")
        await page.keyboard.press("Escape")
        await page.locator(css.menuItem).first().waitFor({ state: "detached" })

        // Two fingers left uncover Archive and Delete.
        const closed = await contentLeft(wrapper())
        await row.hover()
        await swipe(row, [40, 40, 40])
        await waitAttr(wrapper(), "data-swipe", "open")
        // At rest open: the row slid left by exactly the tray's width.
        const rest = await settledOffset(wrapper(), closed)
        if (Math.abs(rest.offset + rest.tray) > 1)
          failures.push(`the row rests at ${rest.offset} px, not -${rest.tray} px`)
        const tray = await wrapper()
          .locator('[data-slot="workspace-swipe-row-action"]')
          .allInnerTexts()
        if (tray.map((each) => each.trim()).join("|") !== "Archive|Delete")
          failures.push(`the swipe uncovered ${JSON.stringify(tray)}`)
        const open = await shot(page, options, engine, "swipe", wrapper())
        if ((await openId(page)) !== before) failures.push("the swipe opened the row")

        // Delete asks once more in place, then removes the row. The tray is
        // the pointer's alone (the row's menu is the keyboard's way to the
        // same actions), so its buttons are found by their slot, not a role.
        const action = (word) =>
          wrapper().locator('[data-slot="workspace-swipe-row-action"]', { hasText: word })
        await action("Delete").click()
        const confirm = wrapper().locator(
          '[data-slot="workspace-swipe-row-action"][data-confirming]',
        )
        await confirm.waitFor({ timeout: 3_000 })
        const asked = await confirm.getAttribute("aria-label")
        if (!asked?.startsWith("Confirm: Delete "))
          failures.push(`the second press reads ${JSON.stringify(asked)}`)
        const confirming = await shot(page, options, engine, "swipe-confirm", wrapper())
        await confirm.click()
        await rowById(page, id).waitFor({ state: "detached", timeout: 3_000 })

        // A long two-finger swipe never archives: the row rests open, held
        // at its tray, and Archive is one press away.
        row = await quietRow(page)
        id = await row.getAttribute("data-session-row")
        await row.hover()
        await swipe(row, [
          3,
          6,
          9,
          ...Array.from({ length: 30 }, (_, at) => (at % 2 ? 9 : 11)),
        ])
        await waitAttr(wrapper(), "data-swipe", "open")
        await page.waitForTimeout(400)
        if (!(await rowById(page, id).count()))
          failures.push("the long swipe archived the row")
        else {
          await action("Archive").click()
          await rowById(page, id).waitFor({ state: "detached", timeout: 5_000 })
        }
        if ((await openId(page)) !== before) failures.push("the long swipe opened a row")
        return { failures, evidence: [open, confirming].filter(Boolean) }
      })
    })
  },
)

/** A row that is not the open one, so opening it would show. */
async function quietRow(page) {
  const row = page
    .locator(`${css.sessionList} [data-session-row]:not([data-selected])`)
    .first()
  await row.waitFor()
  return row
}

function rowById(page, id) {
  return page.locator(`${css.sessionList} [data-session-row="${id}"]`)
}

async function openId(page) {
  const open = page
    .locator(`${css.sessionList} [data-session-row][data-selected]`)
    .first()
  return (await open.count()) ? open.getAttribute("data-session-row") : null
}

async function openMore(page, row) {
  await row.hover()
  await row.locator('[data-session-action="more"]').click()
  await need(page, css.menuItem, "the More menu")
}

/** The row's time and its visible buttons. */
function parts(row) {
  return row.evaluate((element) => {
    const shown = (node) => node && node.getBoundingClientRect().width > 0
    return {
      time: shown(element.querySelector("time")),
      buttons: [...element.querySelectorAll("[data-session-action]")]
        .filter(shown)
        .map((button) => {
          const box = button.getBoundingClientRect()
          return {
            action: button.getAttribute("data-session-action"),
            width: box.width,
            height: box.height,
          }
        }),
    }
  })
}

/**
 * Two fingers on a trackpad: horizontal wheel deltas at the row, back to
 * back as a gesture delivers them.
 */
function swipe(row, deltas) {
  return row.evaluate((element, list) => {
    for (const deltaX of list)
      element.dispatchEvent(
        new WheelEvent("wheel", { deltaX, bubbles: true, cancelable: true }),
      )
  }, deltas)
}

/** Where the row's content starts, from its wrapper's edge. */
function contentLeft(wrapper) {
  return wrapper.evaluate(
    (element) =>
      element.firstElementChild.getBoundingClientRect().left -
      element.getBoundingClientRect().left,
  )
}

/**
 * How far the open row's content has slid from where it rested closed
 * (`closed`, from `contentLeft`), and the tray's width, once it holds still.
 */
async function settledOffset(wrapper, closed) {
  const deadline = Date.now() + 3_000
  let last = null
  for (;;) {
    const now = await wrapper.evaluate((element, from) => {
      const content = element.firstElementChild
      const tray = element.querySelector('[data-slot="workspace-swipe-row-actions"]')
      return {
        offset:
          content.getBoundingClientRect().left -
          element.getBoundingClientRect().left -
          from,
        // The open tray's width as SwipeRow states it: the tray itself spans
        // the row, so a full swipe's leading action can stretch across it.
        tray:
          parseFloat(
            getComputedStyle(
              tray?.querySelector('[data-slot="workspace-swipe-row-action"]') ?? element,
            ).getPropertyValue("--nessa-swipe-open"),
          ) ||
          (tray?.getBoundingClientRect().width ?? 0),
      }
    }, closed)
    if (last && now.offset === last.offset && now.tray === last.tray && now.offset !== 0)
      return now
    if (Date.now() > deadline)
      throw new Error(
        `the open row never held still (${now.offset} px, tray ${now.tray} px)`,
      )
    last = now
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

/** The label's text once it reads `want`, or what it reads at the deadline. */
async function waitText(locator, want) {
  const deadline = Date.now() + 3_000
  for (;;) {
    const now = await locator.innerText()
    if (now === want || Date.now() > deadline) return now
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

/** The session whose row in the session list holds focus, or `null`. */
function focusedRow(page) {
  return page.evaluate(() =>
    document.activeElement?.closest(".workspace-list")
      ? document.activeElement.getAttribute("data-session-row")
      : null,
  )
}

/** The focused row once `holds` says so, or the one focused at the deadline. */
async function waitFocusedRow(page, holds) {
  const deadline = Date.now() + 3_000
  for (;;) {
    const now = await focusedRow(page)
    if (holds(now) || Date.now() > deadline) return now
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

async function waitAttr(locator, name, value) {
  const deadline = Date.now() + 3_000
  for (;;) {
    if ((await locator.getAttribute(name)) === value) return
    if (Date.now() > deadline)
      throw new Error(`${name} never read ${value} (${await locator.getAttribute(name)})`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
}

/** The row, and `below` px under it for a menu; the page's dialog when `target` is null. */
async function shot(page, options, engine, name, target, below = 0) {
  if (!options.shots) return null
  mkdirSync(options.shots, { recursive: true })
  const path = join(options.shots, `${engine}-${name}.png`)
  if (!target) {
    await page.locator('[role="dialog"]').screenshot({ path })
    return path
  }
  const box = await target.boundingBox()
  await page.screenshot({
    path,
    clip: {
      x: box.x - 8,
      y: box.y - 8,
      width: box.width + 16,
      height: box.height + 16 + below,
    },
  })
  return path
}
