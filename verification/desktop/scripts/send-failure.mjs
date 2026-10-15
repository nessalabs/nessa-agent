#!/usr/bin/env node
/** Compact composer errors, complete Copy payload, and identity-preserving recovery. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  { name: "send-failure", defaults: { engine: "chromium,webkit" } },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const surface of ["conversation", "overview"])
        for (const width of [360, 1000]) {
          const target = new URL(
            "/verification/desktop/fixtures/send-failure/index.html",
            url,
          )
          target.searchParams.set("surface", surface)
          const opened = await openPage(browser, { url: target.href, width, height: 800 })
          const page = opened.page
          try {
            const notice = page.locator(css.sendFailure)
            await notice.waitFor()
            const detail = await page.evaluate(() => window.__sendFailure)
            const text = await notice.innerText()
            if (
              !text.includes(`Error sending · ${detail.code}`) ||
              text.includes(detail.diagnostic)
            )
              throw new Error("failure is verbose or loses the actual code")
            const placement = await page.evaluate((selector) => {
              const row = document.querySelector(selector)
              const field = document.querySelector("textarea")
              return (
                row &&
                field &&
                row.getBoundingClientRect().bottom <= field.getBoundingClientRect().top
              )
            }, css.sendFailure)
            if (!placement) throw new Error("failure is not above the composer")
            const overflow = await page.evaluate(
              (selector) =>
                [
                  ...document.querySelectorAll(
                    `${selector}, ${selector} button, .workspace-bubble`,
                  ),
                ].some((element) => {
                  const box = element.getBoundingClientRect()
                  return box.left < 0 || box.right > innerWidth + 1
                }),
              css.sendFailure,
            )
            if (overflow)
              throw new Error("recovery controls or message overflow the actual viewport")

            await page
              .getByRole("button", { name: "Copy error details", exact: true })
              .click()
            await page.waitForFunction(() => window.__sendFailure.copied.length > 0)
            const copied = await page.evaluate(() => window.__sendFailure.copied)
            if (copied !== `${detail.code}\n${detail.diagnostic}`)
              throw new Error("Copy lost diagnostic detail")
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await page.screenshot({
                path: resolve(options.shots, `${engine}-${surface}-${width}.png`),
              })
            }
            await page.getByRole("button", { name: "Send Again", exact: true }).click()
            await page.waitForFunction(() => window.__sendFailure.attempts === 2)
            await notice.waitFor()
            if (
              surface === "conversation" &&
              (await page.locator(`${css.message}[data-role="user"]`).count()) !== 1
            )
              throw new Error("retry duplicated the failed message")
            await page.getByRole("button", { name: "Discard", exact: true }).click()
            await notice.waitFor({ state: "detached" })
            rep.add({
              name: "compact-recovery",
              engine,
              surface,
              width,
              failures: [],
              detail: "above composer; full Copy payload; retry once; discard",
            })
          } finally {
            await opened.close()
          }
        }
      for (const surface of ["setup", "first-use"]) {
        const firstUse = surface === "first-use"
        const attempts = firstUse ? 0 : 1
        const target = new URL(
          `/verification/desktop/fixtures/send-failure/index.html?surface=${surface}`,
          url,
        )
        const opened = await openPage(browser, {
          url: target.href,
          width: 360,
          height: 800,
        })
        try {
          const page = opened.page
          const setup = page.getByRole("region", { name: "Claude setup" })
          const download = page.getByRole("button", {
            name: "Download Claude · 117 MB",
            exact: true,
          })
          await download.waitFor()
          // A production-sized label beside Check setup again must wrap inside
          // the card, never run past it or the viewport.
          const geometry = await setup.evaluate((region) => {
            const card = region.querySelector(".agent-action-card")
            if (!card) return "no setup card"
            const box = card.getBoundingClientRect()
            if (box.left < 0 || box.right > innerWidth + 1) return "card leaves viewport"
            for (const button of card.querySelectorAll("button")) {
              const own = button.getBoundingClientRect()
              if (own.left < box.left - 1 || own.right > box.right + 1)
                return `"${button.textContent}" leaves the card`
              if (button.scrollWidth > button.clientWidth + 1)
                return `"${button.textContent}" is clipped`
            }
            return card.scrollWidth > card.clientWidth + 1 ? "card overflows" : null
          })
          if (geometry) throw new Error(`setup card geometry: ${geometry}`)
          await download.click()
          await page
            .getByText("Downloading and verifying Claude.", { exact: false })
            .waitFor()
          let state = await page.evaluate(() => ({
            attempts: window.__sendFailure.attempts,
            installs: window.__sendFailure.installs,
          }))
          if (state.attempts !== attempts || state.installs.join() !== "claude")
            throw new Error("setup retargeted or automatically retried a send")
          await page.evaluate(() => window.__sendFailure.releaseInstall())
          await page
            .getByRole("button", { name: "Sign in to Claude", exact: true })
            .click()
          if (firstUse) await setup.waitFor({ state: "detached" })
          else
            await page
              .getByText("Ready. Send your message when you are ready.", { exact: true })
              .waitFor()
          state = await page.evaluate(() => ({
            attempts: window.__sendFailure.attempts,
            installs: window.__sendFailure.installs,
          }))
          if (state.attempts !== attempts)
            throw new Error("setup completion automatically replayed the message")
          if (options.shots && !firstUse) {
            await setup.screenshot({
              path: resolve(options.shots, `${engine}-selected-agent-setup.png`),
            })
          }
          if (!firstUse) {
            await page.getByRole("button", { name: "Send Again", exact: true }).click()
            await page.locator(css.sendFailure).waitFor({ state: "detached" })
            if ((await page.locator(`${css.message}[data-role="user"]`).count()) !== 1)
              throw new Error("setup retry duplicated the message")
          }
          rep.add({
            name: firstUse ? "first-use-setup" : "selected-agent-setup",
            engine,
            width: 360,
            failures: [],
            detail:
              "selected runtime only; progress; sign-in; no automatic message replay",
          })
        } finally {
          await opened.close()
        }
      }
    })
  },
)
