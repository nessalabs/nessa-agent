/**
 * The shape every check's main takes: parse options, resolve the page,
 * run, print the result, stop what was started, exit with the status
 * (`statusOf`: a broken contract is 1 whatever else could not run).
 */
import { CannotRun, cli, log, report, resultOfThrown } from "./cli.mjs"
import { target } from "./server.mjs"

export async function main(meta, body) {
  const options = cli(meta)
  const rep = report(meta.name, options)
  let page
  let status
  try {
    page = await target(options)
    await body({ options, rep, url: page.url, mode: page.mode })
  } catch (error) {
    // What was collected before it stays; the error is one more result.
    rep.add(resultOfThrown({ name: meta.name }, error))
    if (!(error instanceof CannotRun)) log(error.stack)
  } finally {
    status = rep.finish({ target: page ? { url: page.url, mode: page.mode } : undefined })
    await page?.close()
  }
  process.exit(status)
}
