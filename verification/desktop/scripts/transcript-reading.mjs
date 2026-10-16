#!/usr/bin/env node
/** Production transcript geometry, settled tools, paced text and return-to-latest. */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, transcriptReading as ui } from "./lib/selectors.mjs"
import { readTranscriptSteps } from "./lib/transcript.mjs"

const meta = {
  name: "transcript-reading",
  summary: "Markdown structure, tool disclosure and a reader-controlled scroll",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/transcript-reading.mjs [options]
Checks: markdown, tools, scoped-tool-evidence, stream, jump, interrupt, wheel, reduced. A dev-only bounded production Transcript.
Every check runs in both engines and layouts unless --quick narrows it.
`,
}

async function open(browser, url, layout) {
  const opened = await openPage(browser, {
    url: new URL(ui.page, url).href,
    layout,
    width: 1000,
    height: 800,
    readySelector: ui.work,
  })
  // CodeBlock highlights lazily after becoming visible, inside an open shadow
  // root. Wait for its actual text rather than the empty light-DOM skeleton.
  await opened.page.locator(ui.code).scrollIntoViewIfNeeded()
  await opened.page.locator(ui.code).locator(ui.codeText).waitFor()
  return opened
}

async function lookBack(page) {
  await page.evaluate((ui) => {
    document.querySelector(ui.scroller).scrollTop = 0
  }, ui)
  await page.waitForFunction(
    (ui) => !document.querySelector(ui.jump)?.hasAttribute("data-hidden"),
    ui,
  )
}

const checks = {
  async markdown(page) {
    const code = await page.locator(ui.code).locator(ui.codeText).textContent()
    const seen = await page.evaluate((ui) => {
      const box = (node) => {
        const r = node.getBoundingClientRect()
        return { top: r.top, bottom: r.bottom, width: r.width, height: r.height }
      }
      const paragraphs = [...document.querySelectorAll(ui.paragraphs)]
      return {
        heading: document.querySelector(ui.heading)?.textContent,
        paragraphs: paragraphs
          .slice(0, 2)
          .map((node) => ({ text: node.textContent, ...box(node) })),
        bullets: [...document.querySelectorAll(ui.bullets)].map(
          (node) => node.textContent,
        ),
        numbered: [...document.querySelectorAll(ui.numbered)].map(
          (node) => node.textContent,
        ),
        links: [...document.querySelectorAll(ui.link)].map((node) => ({
          href: node.getAttribute("href"),
          target: node.getAttribute("target"),
        })),
        scripts: document.querySelector(ui.prose)?.querySelectorAll(ui.forbidden).length,
      }
    }, ui)
    const failures = []
    if (seen.heading !== "A readable reply")
      failures.push("Markdown heading not rendered")
    if (
      JSON.stringify(seen.bullets) !==
      JSON.stringify(["Read the plan", "Check the result"])
    )
      failures.push("Markdown bullets not preserved")
    if (JSON.stringify(seen.numbered) !== JSON.stringify(["Start", "Finish"]))
      failures.push("Markdown numbered list not preserved")
    const [first, second] = seen.paragraphs
    if (
      !first ||
      !second ||
      first.text !== "First paragraph." ||
      !second.text?.startsWith("Second paragraph")
    )
      failures.push("Markdown paragraphs not preserved")
    else if (
      first.height <= 0 ||
      first.width <= 0 ||
      second.height <= 0 ||
      second.width <= 0 ||
      second.top < first.bottom
    )
      failures.push("paragraph blocks overlap or have no painted geometry")
    if (!code?.includes("echo hello"))
      failures.push("fenced code not drawn by code surface")
    if (
      !seen.links.some(
        (link) => link.href === "https://example.com/plan" && link.target === null,
      )
    )
      failures.push("ordinary link missing")
    if (seen.scripts || seen.links.some((link) => /^javascript:/i.test(link.href ?? "")))
      failures.push("agent HTML or executable link reached DOM")
    return { seen, code, failures }
  },
  async tools(page) {
    const trigger = page.locator(ui.work)
    const before = await page.locator(ui.steps).count()
    const label = await trigger.textContent()
    // Refuse the wrong starting state before toggling: clicking an already
    // open disclosure closes it and would only produce an unrelated timeout.
    if (before !== 0)
      return { before, label, failures: ["settled steps are not collapsed"] }
    await trigger.click()
    await page.locator(ui.steps).first().waitFor()
    const steps = await page.locator(ui.steps).allTextContents()
    const expanded = await trigger.getAttribute("aria-expanded")
    return {
      before,
      label,
      steps,
      expanded,
      failures: [
        ...(before === 0 ? [] : ["settled steps are not collapsed"]),
        ...(label === "Ran 2 tools · 1 read, 1 edited" ? [] : ["tool summary differs"]),
        ...(steps.length === 2 &&
        steps[0].includes("plan.md") &&
        steps[1].includes("result.md") &&
        expanded === "true"
          ? []
          : ["expanding lost the actual steps"]),
      ],
    }
  },
  async "scoped-tool-evidence"(page) {
    const messages = page.locator(css.agentMessage)
    const owner = messages.filter({ has: page.locator(ui.work) })
    const first = await readTranscriptSteps(owner)
    const again = await readTranscriptSteps(owner)
    const zero = await readTranscriptSteps(
      messages.filter({ hasText: "No such transcript owner" }),
    )
    const ambiguous = await readTranscriptSteps(messages)
    const wrong = await readTranscriptSteps(
      messages.filter({ hasText: "Saved paragraph 0." }),
    )
    const count = await messages.count()
    const failures = [...first.failures, ...again.failures]
    const expected = JSON.stringify(["Read", "Edited"])
    if (
      JSON.stringify(first.labels) !== expected ||
      JSON.stringify(again.labels) !== expected
    )
      failures.push(
        "scoped reads did not preserve the actual Read and Edited tool titles",
      )
    if (
      first.disclosures.length !== 1 ||
      first.disclosures[0].before !== "false" ||
      first.disclosures[0].expanded !== "true"
    )
      failures.push("the scoped read did not open its collapsed activity")
    if (
      again.disclosures.length !== 1 ||
      again.disclosures[0].before !== "true" ||
      again.disclosures[0].expanded !== "true"
    )
      failures.push("reading an already open activity toggled it or lost it")
    if (
      zero.labels.length !== 0 ||
      zero.failures.length !== 1 ||
      !zero.failures[0].includes("0 agent messages")
    )
      failures.push("the read did not refuse its missing owner")
    if (
      count !== 36 ||
      ambiguous.labels.length !== 0 ||
      ambiguous.failures.length !== 1 ||
      !ambiguous.failures[0].includes("36 agent messages")
    )
      failures.push("the read did not refuse all 36 ambiguous owners")
    if (
      wrong.failures.length !== 0 ||
      wrong.labels.length !== 0 ||
      wrong.disclosures.length !== 0
    )
      failures.push("a unique message without tools borrowed another message's evidence")
    return { seen: { count, first, again, zero, ambiguous, wrong }, failures }
  },
  async stream(page) {
    await page.getByRole("button", { name: ui.start, exact: true }).click()
    await page.waitForFunction(
      (selector) =>
        document.querySelector(selector)?.textContent?.trim() === "A streamed reply",
      ui.live,
    )
    // Observe the last chunk through its reveal and fade, rather than stopping
    // when it first appears. End after 1.5s of stable final text (kit fade1s).
    const seen = await page.evaluate(async (ui) => {
      const expected = "A streamed reply keeps its final words."
      const button = [...document.querySelectorAll(ui.controls)].find(
        (node) => node.textContent === ui.finish,
      )
      if (!button) throw new Error("fixture finish control missing")
      button.click()
      let previous = 0
      let backwards = false
      let stableAt = null
      const start = performance.now()
      let text = ""
      let samples = 0
      while (performance.now() - start < 8000) {
        await new Promise(requestAnimationFrame)
        text = document.querySelector(ui.live)?.textContent ?? ""
        backwards ||= text.length < previous
        previous = text.length
        samples++
        if (text === expected) stableAt ??= performance.now()
        else stableAt = null
        if (stableAt !== null && performance.now() - stableAt >= 1500) break
      }
      return {
        text,
        backwards,
        samples,
        stableMs: stableAt === null ? 0 : performance.now() - stableAt,
        elapsedMs: performance.now() - start,
        spans: document.querySelector(ui.live)?.querySelectorAll("span").length,
      }
    }, ui)
    return {
      seen,
      failures: [
        ...(seen.text === "A streamed reply keeps its final words."
          ? []
          : ["final streamed text did not settle within8s"]),
        ...(seen.backwards ? ["streamed text went backwards"] : []),
        ...(seen.stableMs >= 1500 ? [] : ["final text was not stable for1.5s"]),
        ...(seen.spans === 0 ? [] : ["finished prose retains animation spans"]),
      ],
    }
  },
  async jump(page) {
    await lookBack(page)
    const returned = await page.evaluate(async (ui) => {
      const scroller = document.querySelector(ui.scroller)
      const duration = getComputedStyle(scroller)
        .getPropertyValue("--desktop-slow")
        .trim()
      const durationMs =
        Number.parseFloat(duration) * (duration.endsWith("ms") ? 1 : 1000)
      const start = performance.now()
      const from = scroller.scrollTop
      const end = scroller.scrollHeight - scroller.clientHeight
      document.querySelector(ui.jump).click()
      const samples = []
      while (performance.now() - start < 1200) {
        await new Promise(requestAnimationFrame)
        samples.push(scroller.scrollTop)
        // Pixel rounding can reach zero distance before the animation's last
        // callback; wait through the published duration, not that early pixel.
        if (
          performance.now() - start >= durationMs &&
          scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= 1
        )
          break
      }
      return {
        from,
        end,
        range: end - from,
        duration,
        samples,
        intermediate: samples.filter(
          (position) => position > from + 1 && position < end - 1,
        ),
        monotonic: samples.every(
          (position, index) => position >= (index === 0 ? from : samples[index - 1]) - 1,
        ),
        elapsedMs: performance.now() - start,
        distance: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
      }
    }, ui)
    await page.waitForFunction(
      (ui) => document.querySelector(ui.jump)?.hasAttribute("data-hidden"),
      ui,
    )
    return {
      returned,
      failures:
        returned.distance <= 1 &&
        returned.range > 1 &&
        returned.intermediate.length > 0 &&
        returned.monotonic
          ? []
          : ["animated return did not traverse real scroll geometry"],
    }
  },
  async interrupt(page) {
    await lookBack(page)
    const interrupted = await page.evaluate(async (ui) => {
      const scroller = document.querySelector(ui.scroller)
      document.querySelector(ui.jump).click()
      await new Promise(requestAnimationFrame)
      const reached = scroller.scrollTop
      // Synthetic control-event accounting isolates the cancellation path
      // from native wheel displacement. Trusted input has a separate check.
      scroller.dispatchEvent(new WheelEvent("wheel", { deltaY: -10, bubbles: true }))
      const start = performance.now()
      let drift = 0
      while (performance.now() - start < 400) {
        await new Promise(requestAnimationFrame)
        drift = Math.max(drift, Math.abs(scroller.scrollTop - reached))
      }
      return { reached, drift, end: scroller.scrollHeight - scroller.clientHeight }
    }, ui)
    return {
      interrupted,
      failures:
        interrupted.reached < interrupted.end - 1 && interrupted.drift <= 1
          ? []
          : ["synthetic wheel control-event interruption did not retain its position"],
    }
  },
  async wheel(page) {
    await lookBack(page)
    const bounds = await page.locator(ui.scroller).boundingBox()
    if (!bounds) throw new CannotRun("transcript has no wheel-input geometry")
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
    await page.evaluate(async (ui) => {
      const scroller = document.querySelector(ui.scroller)
      scroller.addEventListener(
        "wheel",
        (event) => {
          scroller.dataset.verificationWheelAt = String(scroller.scrollTop)
          scroller.dataset.verificationWheelTrusted = String(event.isTrusted)
        },
        { once: true, capture: true },
      )
      document.querySelector(ui.jump).click()
      await new Promise(requestAnimationFrame)
      await new Promise(requestAnimationFrame)
    }, ui)
    await page.mouse.wheel(0, -2400)
    const nativeWheel = await page.evaluate(async (ui) => {
      const scroller = document.querySelector(ui.scroller)
      const start = performance.now()
      let minimum = scroller.scrollTop
      // Allow native wheel kinetics; only direction and the final refusal to
      // continue toward the bottom are asserted, not a zero-drift position.
      while (performance.now() - start < 500) {
        await new Promise(requestAnimationFrame)
        minimum = Math.min(minimum, scroller.scrollTop)
      }
      return {
        at: Number(scroller.dataset.verificationWheelAt ?? "NaN"),
        trusted: scroller.dataset.verificationWheelTrusted === "true",
        minimum,
        final: scroller.scrollTop,
        end: scroller.scrollHeight - scroller.clientHeight,
      }
    }, ui)
    return {
      nativeWheel,
      failures:
        nativeWheel.trusted &&
        nativeWheel.at > 1 &&
        nativeWheel.at < nativeWheel.end - 1 &&
        nativeWheel.minimum < nativeWheel.at - 1 &&
        nativeWheel.final < nativeWheel.end - 24
          ? []
          : [
              "trusted upward wheel was not delivered mid-return or the return continued to the bottom",
            ],
    }
  },
  async reduced(page) {
    await lookBack(page)
    // Consume the published motion state and its computed CSS token, without
    // overriding the token or changing the production animation code.
    const reduced = await page.evaluate((ui) => {
      document.documentElement.dataset.motion = "reduced"
      const scroller = document.querySelector(ui.scroller)
      const duration = getComputedStyle(scroller)
        .getPropertyValue("--desktop-slow")
        .trim()
      document.querySelector(ui.jump).click()
      return {
        duration,
        distance: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
      }
    }, ui)
    return {
      reduced,
      failures:
        reduced.duration === "0ms" && reduced.distance <= 1
          ? []
          : ["published reduced-motion state did not return immediately"],
    }
  },
}

await main(meta, async ({ options, rep, url }) => {
  const html = await fetch(new URL(ui.page, url)).then((response) => response.text())
  if (!html.includes(`<title>${ui.title}</title>`))
    throw new CannotRun("transcript reading fixture requires --mode dev")
  const only = options.only?.split(",")
  if (only?.some((name) => !Object.hasOwn(checks, name)))
    throw new CannotRun("unknown transcript-reading check")
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const [name, check] of Object.entries(checks)) {
        if (only && !only.includes(name)) continue
        await attempt(rep, { name, engine, layout }, async () => {
          const opened = await open(browser, url, layout)
          try {
            const result = await check(opened.page)
            if (options.shots && ["markdown", "tools", "jump"].includes(name)) {
              mkdirSync(options.shots, { recursive: true })
              result.screenshot = join(
                options.shots,
                `transcript-reading-fixture-${name}-${engine}-${layout}.png`,
              )
              await opened.page.locator(ui.ready).screenshot({ path: result.screenshot })
            }
            await opened.settleRequests()
            return result
          } finally {
            await opened.close()
          }
        })
      }
  })
})
