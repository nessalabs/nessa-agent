/**
 * The shape every check's main takes: parse options, resolve the page,
 * run, print the result, stop what was started, exit with the status
 * (`statusOf`: a broken contract is 1 whatever else could not run).
 *
 * `resolve` is how the page is found: `target` (the dev server, a
 * production preview, or `--url`) unless the check starts its own. Whatever
 * it resolves with is handed to `body` as `target`, and its `close` runs
 * at the end whatever happened.
 */
import { cli, report, resultOfThrown } from "./cli.mjs"
import { target } from "./server.mjs"

export async function main(meta, body, resolve = target) {
  const options = cli(meta)
  const rep = report(meta.name, options)
  let page
  let status
  try {
    page = await resolve(options)
    await body({ options, rep, url: page.url, mode: page.mode, target: page })
  } catch (error) {
    // What was collected before it stays; the error is one more result.
    rep.add(resultOfThrown({ name: meta.name }, error))
  } finally {
    status = rep.finish({ target: page ? { url: page.url, mode: page.mode } : undefined })
    await page?.close()
  }
  process.exit(status)
}
