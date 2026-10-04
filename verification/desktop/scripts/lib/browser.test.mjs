/**
 * `browser.mjs`'s recording of a request the page reports failed: one test at
 * least per row F1–F5 of #485's design (and its amendment), each asserting
 * where the line went — `harmless`, `errors`, or neither.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { recordFailedRequest } from "./browser.mjs"

const PAGE = "http://127.0.0.1:1438/desktop.html?gateway"
const RESOURCE = "http://127.0.0.1:1438/mcp-resources"

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

describe("recordFailedRequest", () => {
  it("F1: the window's own /mcp-resources, aborted after a 200, is harmless only, and labelled", () => {
    const { errors, harmless } = recorded(request())
    assert.deepEqual(errors, [])
    assert.deepEqual(harmless, [
      `requestfailed: ${RESOURCE} net::ERR_ABORTED (aborted after a 200; the bytes are checked by renders, #485)`,
    ])
  })

  it("F2: the same URL with no response (aborted before headers) is an error only", () => {
    anError(request({ status: null }), `requestfailed: ${RESOURCE} net::ERR_ABORTED`)
  })

  it("F3: the same URL with any status other than 200 is an error only", () => {
    for (const status of [204, 206, 304, 400, 404, 413, 500]) {
      anError(request({ status }), `requestfailed: ${RESOURCE} net::ERR_ABORTED`)
    }
  })

  it("F4: any other URL is an error only, as before", () => {
    for (const url of [
      // Another path on the page's origin.
      "http://127.0.0.1:1438/browser/conversations",
      "http://127.0.0.1:1438/mcp-resources/extra",
      "http://127.0.0.1:1438/x/mcp-resources",
      // The same path on another origin: the sandbox proxy's, another port.
      "http://127.0.0.1:1439/mcp-resources",
      "http://localhost:1438/mcp-resources",
      "https://127.0.0.1:1438/mcp-resources",
      "not a url",
    ]) {
      anError(request({ url }), `requestfailed: ${url} net::ERR_ABORTED`)
    }
  })

  it("F4: any other error text is an error only, as before", () => {
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

  it("F4: a page with no origin to resolve the resource on leaves every failure an error", () => {
    for (const pageUrl of ["about:blank", ""]) {
      anError(request(), `requestfailed: ${RESOURCE} net::ERR_ABORTED`, pageUrl)
    }
  })

  it("F5: the same URL with a query string or a fragment is an error only", () => {
    for (const url of [`${RESOURCE}?ticket=x`, `${RESOURCE}?`, `${RESOURCE}#x`]) {
      anError(request({ url }), `requestfailed: ${url} net::ERR_ABORTED`)
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
