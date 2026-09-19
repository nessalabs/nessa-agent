/**
 * Reading what an HTTP request asked for.
 *
 * Its own module because the harness it serves starts a server the moment it is
 * imported, so nothing can test anything that lives inside it — which is how
 * this ended up in `updater-manifest.mjs`, a module about the shape of a release
 * manifest, where it had no business being either.
 */

/**
 * The path a request meant, or undefined when it cannot be read.
 *
 * Both steps of reading a request target throw, and neither is caught in a
 * Node request handler — an exception there ends the process. `new URL` throws
 * on a target that is not one (`//[`), and `decodeURIComponent` throws on a
 * malformed escape (`/%ZZ`). A stray request of either shape would end the
 * harness in the middle of a run somebody is watching, so the two live behind
 * this one door and a target that cannot be read is simply not a request for
 * anything served: it falls through to the same 404 as any unknown path.
 *
 * @param {string | undefined} target the raw request target
 * @param {string} origin what a path-only target is resolved against
 * @returns {string | undefined} the decoded pathname
 */
export function requestedPath(target, origin) {
  try {
    return decodeURIComponent(new URL(target ?? "", origin).pathname)
  } catch {
    return undefined
  }
}
