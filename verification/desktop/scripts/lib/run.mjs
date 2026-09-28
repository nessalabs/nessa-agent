/**
 * The shape every check's main takes: parse options, resolve the page,
 * run, print the result, stop what was started, exit with the status.
 */
import { CannotRun, cli, log, report } from "./cli.mjs"
import { target } from "./server.mjs"

export async function main(meta, body) {
  const options = cli(meta)
  const rep = report(meta.name, options)
  let page
  let status
  try {
    page = await target(options)
    await body({ options, rep, url: page.url, mode: page.mode })
    status = rep.finish({ target: { url: page.url, mode: page.mode } })
  } catch (error) {
    const cannotRun = error instanceof CannotRun
    rep.add({
      name: meta.name,
      cannotRun: true,
      error: cannotRun ? `could not run: ${error.message}` : error.message,
    })
    rep.finish({ target: page ? { url: page.url, mode: page.mode } : undefined })
    if (!cannotRun) log(error.stack)
    status = 2
  } finally {
    await page?.close()
  }
  process.exit(status)
}
