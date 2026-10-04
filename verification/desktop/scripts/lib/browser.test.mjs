/**
 * `browser.mjs`'s recording of a request the page reports failed: one test at
 * least per row F1′ and F2–F4 of #485's design (amendment 3), each asserting
 * where the line went — `harmless`, `errors`, or neither.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { recordFailedRequest } from "./browser.mjs"

const PAGE = "http://127.0.0.1:1438/desktop.html?gateway"
const RESOURCE = "http://127.0.0.1:1438/mcp-resources"
const CHECK = "http://127.0.0.1:1438/browser/check"

/** A failed request as Playwright's `requestfailed` hands it over. */
const request = ({
  url = RESOURCE,
  errorText = "net::ERR_ABORTED",
  status = 200,
} = {}) => ({
  url: () => url,
  failure: () => (errorText === null ? null : { errorText }),
  existingResponse: () => (status === null ? null : { status: () => status }),
})

/** Where `recordFailedRequest` put the request's line. */
function recorded(failed, pageUrl = PAGE) {
  const into = { errors: [], harmless: [] }
  recordFailedRequest(failed, pageUrl, into)
  return into
}

/** Asserts the request is recorded as an error, alone, with `line`. */
const anError = (failed, line, pageUrl) =>
  assert.deepEqual(recorded(failed, pageUrl), { errors: [line], harmless: [] })

/** Asserts the request is recorded as harmless, alone, labelled with its status. */
const harmlessOnly = (failed, url, status) =>
  assert.deepEqual(recorded(failed), {
    errors: [],
    harmless: [
      `requestfailed: ${url} net::ERR_ABORTED (aborted after a ${status} response, #485)`,
    ],
  })

describe("recordFailedRequest", () => {
  it("F1′: the window's /mcp-resources, aborted after a 200 (read through a bounded reader), is harmless only", () => {
    harmlessOnly(request(), RESOURCE, 200)
  })

  it("F1′: the window's /browser/check, aborted after a 204 (its body never read), is harmless only", () => {
    harmlessOnly(request({ url: CHECK, status: 204 }), CHECK, 204)
  })

  it("F1′: any path, query or fragment on the page's own origin, after any 2xx, is harmless only", () => {
    for (const [url, status] of [
      ["http://127.0.0.1:1438/browser/conversations", 200],
      [`${RESOURCE}?x=1#y`, 206],
      ["http://127.0.0.1:1438/", 299],
    ]) {
      harmlessOnly(request({ url, status }), url, status)
    }
  })

  it("F2: a response never received (aborted before headers) is an error only", () => {
    for (const url of [RESOURCE, CHECK]) {
      anError(request({ url, status: null }), `requestfailed: ${url} net::ERR_ABORTED`)
    }
  })

  it("F3: a response outside 2xx is an error only, at both bounds", () => {
    for (const status of [100, 199, 300, 304, 400, 401, 404, 413, 500]) {
      anError(request({ status }), `requestfailed: ${RESOURCE} net::ERR_ABORTED`)
    }
  })

  it("F4: another origin is an error only", () => {
    for (const url of [
      // The sandbox proxy's origin: another port.
      "http://127.0.0.1:1439/mcp-resources",
      "http://localhost:1438/mcp-resources",
      "https://127.0.0.1:1438/mcp-resources",
      "data:text/plain,x",
      "not a url",
    ]) {
      anError(request({ url }), `requestfailed: ${url} net::ERR_ABORTED`)
    }
  })

  it("F4: any other error text is an error only", () => {
    for (const errorText of [
      "net::ERR_FAILED",
      "net::ERR_CONNECTION_REFUSED",
      "net::ERR_ABORTED ",
      "Load request cancelled",
      "",
    ]) {
      anError(request({ errorText }), `requestfailed: ${RESOURCE} ${errorText}`)
    }
    anError(request({ errorText: null }), `requestfailed: ${RESOURCE} `)
  })

  it("F4: a page with no origin (about:blank) leaves every failure an error", () => {
    for (const pageUrl of ["about:blank", "", "data:text/html,x"]) {
      anError(request(), `requestfailed: ${RESOURCE} net::ERR_ABORTED`, pageUrl)
      anError(
        request({ url: "data:text/plain,x" }),
        "requestfailed: data:text/plain,x net::ERR_ABORTED",
        pageUrl,
      )
    }
  })

  it("records nothing for the fresh browser's /favicon.ico, as before", () => {
    for (const status of [200, 404, null]) {
      assert.deepEqual(
        recorded(request({ url: "http://127.0.0.1:1438/favicon.ico", status })),
        { errors: [], harmless: [] },
      )
    }
  })
})
