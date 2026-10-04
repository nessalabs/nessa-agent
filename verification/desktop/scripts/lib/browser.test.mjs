/**
 * `browser.mjs`'s reading of a request the page reports failed: one test at
 * least per row F1–F4 of #485's design.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import { failedRequest } from "./browser.mjs"

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

describe("failedRequest", () => {
  it("F1: the window's own /mcp-resources, aborted after a 200, is harmless and labelled", () => {
    const read = failedRequest(request(), PAGE)
    assert.equal(read.error, undefined)
    assert.match(
      read.harmless,
      /^requestfailed: http:\/\/127\.0\.0\.1:1438\/mcp-resources net::ERR_ABORTED /,
    )
    assert.match(
      read.harmless,
      /Chromium reports a body read through a reader as aborted, #485/,
    )
  })

  it("F2: the same URL with no response (aborted before headers) is an error", () => {
    assert.deepEqual(failedRequest(request({ status: null }), PAGE), {
      error: `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
    })
  })

  it("F3: the same URL with any status other than 200 is an error", () => {
    for (const status of [204, 206, 304, 400, 404, 413, 500]) {
      assert.deepEqual(failedRequest(request({ status }), PAGE), {
        error: `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
      })
    }
  })

  it("F4: any other URL is an error, as before", () => {
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
      assert.deepEqual(failedRequest(request({ url }), PAGE), {
        error: `requestfailed: ${url} net::ERR_ABORTED`,
      })
    }
  })

  it("F4: any other error text is an error, as before", () => {
    for (const errorText of [
      "net::ERR_FAILED",
      "net::ERR_CONNECTION_REFUSED",
      "net::ERR_ABORTED ",
      "Load request cancelled",
      "",
    ]) {
      assert.deepEqual(failedRequest(request({ errorText }), PAGE), {
        error: `requestfailed: ${RESOURCE} ${errorText}`,
      })
    }
    assert.deepEqual(failedRequest(request({ errorText: null }), PAGE), {
      error: `requestfailed: ${RESOURCE} `,
    })
  })

  it("F4: a page with no URL to compare to leaves every failure an error", () => {
    assert.ok(failedRequest(request(), "about:blank").error)
    assert.ok(failedRequest(request(), "").error)
  })
})
