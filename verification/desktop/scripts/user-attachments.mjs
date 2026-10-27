#!/usr/bin/env node
/** Input images and linked files survive desktop projection and reopening. */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { userAttachments as ui } from "./lib/selectors.mjs"

await main(
  {
    name: "user-attachments",
    summary: "desktop input references retain metadata without byte lookup",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of options.quick ? [600] : [360, 600, 1440]) {
        const opened = await openPage(browser, {
          url: new URL(ui.page, url).href,
          readySelector: ui.ready,
          width,
          lines: { width },
        })
        try {
          const { page } = opened
          for (const reopen of [false, true]) {
            if (reopen) {
              await page.reload()
              await page.locator(ui.ready).waitFor()
            }
            for (const phase of ["committed", "pending", "restored"]) {
              for (const kind of ["image", "file", "upload", "mixed"]) {
                await attempt(
                  rep,
                  {
                    engine,
                    width,
                    name: `${phase}:${kind}:${reopen ? "reopen" : "open"}`,
                  },
                  async () => {
                    const scene = page.locator(ui.case(phase, kind))
                    const seen = await scene.evaluate(
                      (node, selectors) => ({
                        titles: [...node.querySelectorAll(selectors.tiles)].map(
                          (item) => item.title,
                        ),
                        lists: [...node.querySelectorAll(selectors.lists)].map((list) =>
                          list.getAttribute("aria-label"),
                        ),
                        pictures: node.querySelectorAll(selectors.pictures).length,
                        controls: node.querySelectorAll(selectors.controls).length,
                        text: node.querySelector(selectors.text)?.textContent ?? "",
                        overflow: node.scrollWidth > node.clientWidth + 1,
                        surfaceWidth: node.getBoundingClientRect().width,
                      }),
                      {
                        tiles: ui.tiles,
                        lists: ui.lists,
                        text: ui.text,
                        pictures: ui.pictures,
                        controls: ui.controls,
                      },
                    )
                    const expected =
                      kind === "image"
                        ? ["PNG image, 2 KiB"]
                        : kind === "file"
                          ? ["/workspace/quarterly [report].csv"]
                          : kind === "upload"
                            ? ["meeting notes.pdf"]
                            : [
                                "PNG image, 2 KiB",
                                "/workspace/quarterly [report].csv",
                                "meeting notes.pdf",
                              ]
                    return {
                      ...seen,
                      failures: [
                        ...(JSON.stringify(seen.titles) !== JSON.stringify(expected)
                          ? [
                              `attachment metadata ${JSON.stringify(seen.titles)}, expected ${JSON.stringify(expected)}`,
                            ]
                          : []),
                        ...(seen.lists[0] !==
                        `${expected.length} attachment${expected.length === 1 ? "" : "s"}`
                          ? ["attachment list is not labelled"]
                          : []),
                        ...(seen.text !==
                        (kind === "mixed" ? "Inspect this [report], please." : "")
                          ? ["input text changed or image-only input has text"]
                          : []),
                        ...(seen.pictures || seen.controls
                          ? [
                              "metadata placeholder presents bytes or actions it does not have",
                            ]
                          : []),
                        ...(seen.overflow || seen.surfaceWidth <= 0
                          ? ["input does not fit its visible surface"]
                          : []),
                      ],
                    }
                  },
                )
              }
            }
          }
          if (options.shots && width === 600) {
            mkdirSync(options.shots, { recursive: true })
            await page
              .locator(ui.case("restored", "mixed"))
              .screenshot({ path: join(options.shots, `${engine}-restored-input.png`) })
          }
          await opened.settleRequests()
        } finally {
          await opened.close()
        }
      }
    })
  },
)
