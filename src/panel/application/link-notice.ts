/**
 * What a link that did not open puts on the panel.
 *
 * Links leave Nessa rather than opening in it: the panel is a floating bar with
 * no address bar and no back button, and the host's commands are granted to
 * this window, so a page fetched by a tool has no business loading inside it.
 * The host decides (see `src-tauri/src/links.rs`) and this is what the person
 * reads when the decision was no.
 *
 * Both reasons produce a click that does nothing, which on its own is
 * indistinguishable from a broken app, so both get a sentence:
 *
 * ```text
 *   refused       ──▶ the scheme is not one a link here may open
 *   opener-failed ──▶ it was a web address, and the browser would not start
 * ```
 *
 * A function rather than branches in the surface, for the reason
 * `update-surface.ts` is one: the wording is the part worth testing, and the
 * event that drives it comes from a host process.
 */

import type { LinkNotOpened } from "../../host"

/** One notice above the composer. */
export interface LinkNotice {
  title: string
  description: string
}

/**
 * The link itself, shortened enough to sit in a notice in a 420pt panel.
 *
 * The person is owed which link did nothing — one of several in a message may
 * have been refused — but not a thousand characters of query string. The tail
 * is dropped rather than the middle elided: the scheme and host are the part
 * that explains the refusal.
 */
function shorten(url: string): string {
  const limit = 72
  return url.length > limit ? `${url.slice(0, limit - 1)}…` : url
}

/** What to put on screen for a link the host did not open. */
export function linkNotice(link: LinkNotOpened): LinkNotice {
  if (link.reason === "refused")
    return {
      title: "Link not opened",
      description: `Nessa opens web and mail links in your browser, and does not open ${shorten(
        link.url,
      )}.`,
    }
  return {
    title: "Browser did not start",
    description: `Nessa could not hand ${shorten(link.url)} to your browser. Opening it there yourself will work.`,
  }
}
