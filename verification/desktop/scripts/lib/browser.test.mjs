/**
 * `browser.mjs`'s recording of a request the page reports failed: one test at
 * least per row of #473 and #485, each asserting where the line went —
 * `harmless`, `errors`, or neither.
 */
import assert from "node:assert/strict"
import { describe, it } from "node:test"

import {
  enqueueSizeReport,
  liveMountResourceAbort,
  reclassifyDeliveredAbort,
  recordFailedRequest,
  runAndClose,
  settleSizeReports,
} from "./browser.mjs"

const PAGE = "http://127.0.0.1:1438/desktop.html?gateway"
const RESOURCE = "http://127.0.0.1:1438/mcp-resources"
const CHECK = "http://127.0.0.1:1438/browser/check"

/** A failed request as Playwright's `requestfailed` hands it over. */
const request = ({
  url = RESOURCE,
  errorText = "net::ERR_ABORTED",
  status = 200,
  contentLength = 4095,
  responseBodySize = 4095,
  sizes = undefined,
} = {}) => ({
  url: () => url,
  failure: () => (errorText === null ? null : { errorText }),
  existingResponse: () =>
    status === null
      ? null
      : {
          status: () => status,
          headers: () =>
            contentLength === null ? {} : { "content-length": String(contentLength) },
        },
  sizes: () => (sizes === undefined ? { responseBodySize } : sizes),
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

/** Asserts the request is recorded as harmless, alone, with `label`. */
const harmlessOnly = (failed, label) =>
  assert.deepEqual(recorded(failed), { errors: [], harmless: [label] })

const fullBody = (url) =>
  `requestfailed: ${url} net::ERR_ABORTED (aborted after a 200 response, full body, #473)`
const emptyBody = (url) =>
  `requestfailed: ${url} net::ERR_ABORTED (aborted after a 204 response, #485)`

describe("recordFailedRequest", () => {
  it("a 200 whose content-length was fully delivered is harmless only (#473)", () => {
    harmlessOnly(request(), fullBody(RESOURCE))
    harmlessOnly(
      request({ url: `${RESOURCE}?x=1#y`, contentLength: 10, responseBodySize: 10 }),
      fullBody(`${RESOURCE}?x=1#y`),
    )
  })

  it("a 200 is an error when the body is short, or the size was not reported", () => {
    anError(
      request({ responseBodySize: 4094 }),
      `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
    )
    anError(
      request({ contentLength: null, responseBodySize: 4095 }),
      `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
    )
    anError(
      request({ sizes: Promise.resolve({ responseBodySize: 4095 }) }),
      `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
    )
  })

  it("a later size report moves that same line to harmless, and a short body does not", () => {
    const into = {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    }
    reclassifyDeliveredAbort(
      request({ responseBodySize: 100 }),
      PAGE,
      { responseBodySize: 100 },
      into,
    )
    assert.deepEqual(into, {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    })
    reclassifyDeliveredAbort(request(), PAGE, { responseBodySize: 4095 }, into)
    assert.deepEqual(into, { errors: [], harmless: [fullBody(RESOURCE)] })
  })

  it("a later size report does not excuse another origin or another error", () => {
    const other = "http://127.0.0.1:1439/mcp-resources"
    const cross = {
      errors: [`requestfailed: ${other} net::ERR_ABORTED`],
      harmless: [],
    }
    reclassifyDeliveredAbort(
      request({ url: other }),
      PAGE,
      { responseBodySize: 4095 },
      cross,
    )
    assert.deepEqual(cross, {
      errors: [`requestfailed: ${other} net::ERR_ABORTED`],
      harmless: [],
    })
    const failed = {
      errors: [`requestfailed: ${RESOURCE} net::ERR_FAILED`],
      harmless: [],
    }
    reclassifyDeliveredAbort(
      request({ errorText: "net::ERR_FAILED" }),
      PAGE,
      { responseBodySize: 4095 },
      failed,
    )
    assert.deepEqual(failed, {
      errors: [`requestfailed: ${RESOURCE} net::ERR_FAILED`],
      harmless: [],
    })
  })

  it("only an unlabelled same-origin /mcp-resources abort is the mount-went-live case", () => {
    assert.equal(
      liveMountResourceAbort(`requestfailed: ${RESOURCE} net::ERR_ABORTED`, PAGE),
      true,
    )
    assert.equal(
      liveMountResourceAbort(`requestfailed: ${RESOURCE}?x=1 net::ERR_ABORTED`, PAGE),
      true,
    )
    assert.equal(
      liveMountResourceAbort(
        `requestfailed: http://127.0.0.1:1439/mcp-resources net::ERR_ABORTED`,
        PAGE,
      ),
      false,
    )
    assert.equal(liveMountResourceAbort(fullBody(RESOURCE), PAGE), false)
    assert.equal(
      liveMountResourceAbort(`requestfailed: ${CHECK} net::ERR_ABORTED`, PAGE),
      false,
    )
    assert.equal(
      liveMountResourceAbort(`requestfailed: ${RESOURCE} net::ERR_FAILED`, PAGE),
      false,
    )
    assert.equal(
      liveMountResourceAbort(`requestfailed: ${RESOURCE} net::ERR_ABORTED`, ""),
      false,
    )
  })

  it("F1′: the window's /browser/check, aborted after a 204 (its body never read), is harmless only", () => {
    harmlessOnly(
      request({ url: CHECK, status: 204, contentLength: null }),
      emptyBody(CHECK),
    )
  })

  it("a 2xx other than a full 200 or an empty 204 is an error", () => {
    for (const status of [201, 206, 299]) {
      anError(
        request({ status, contentLength: 4, responseBodySize: 4 }),
        `requestfailed: ${RESOURCE} net::ERR_ABORTED`,
      )
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

describe("settleSizeReports", () => {
  it("lets a full-body report leave errors before the caller reads them", async () => {
    const into = {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    }
    let report
    const pending = [
      new Promise((resolve) => {
        report = resolve
      }).then((sizes) => reclassifyDeliveredAbort(request(), PAGE, sizes, into)),
    ]
    const waiting = settleSizeReports(pending)
    assert.deepEqual(into.harmless, [])
    report({ responseBodySize: 4095 })
    await waiting
    assert.deepEqual(into, { errors: [], harmless: [fullBody(RESOURCE)] })
  })

  it("leaves the line an error when the size report does not arrive", async () => {
    const into = {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    }
    const pending = [new Promise(() => {})]
    await settleSizeReports(pending, 30)
    assert.deepEqual(into, {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    })
  })

  it("a late full body does not excuse a later identical line", async () => {
    const line = `requestfailed: ${RESOURCE} net::ERR_ABORTED`
    const into = { errors: [line], harmless: [] }
    let report
    const pending = []
    pending.push(
      enqueueSizeReport(
        new Promise((resolve) => {
          report = resolve
        }),
        (sizes) => reclassifyDeliveredAbort(request(), PAGE, sizes, into),
      ),
    )
    await settleSizeReports(pending, 30)
    // The step already copied the first line out. A second abort of the same
    // url is a different failure, even when the first size finally arrives.
    into.errors.splice(0)
    into.errors.push(line)
    report({ responseBodySize: 4095 })
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.deepEqual(into, { errors: [line], harmless: [] })
  })

  it("leaves the line an error when the body is short", async () => {
    const into = {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    }
    const pending = [
      Promise.resolve({ responseBodySize: 100 }).then((sizes) =>
        reclassifyDeliveredAbort(request({ responseBodySize: 100 }), PAGE, sizes, into),
      ),
    ]
    await settleSizeReports(pending)
    assert.deepEqual(into, {
      errors: [`requestfailed: ${RESOURCE} net::ERR_ABORTED`],
      harmless: [],
    })
  })
})

describe("runAndClose", () => {
  it("keeps the body's error when close also rejects", async () => {
    let closed = false
    const browser = {
      close: async () => {
        closed = true
        throw new Error("close failed")
      },
    }
    await assert.rejects(
      runAndClose(browser, async () => {
        throw new Error("step failed")
      }),
      { message: "step failed" },
    )
    assert.equal(closed, true)
  })

  it("resolves the body's value when close rejects", async () => {
    let closed = false
    const browser = {
      close: async () => {
        closed = true
        throw new Error("close failed")
      },
    }
    assert.equal(await runAndClose(browser, async () => "done"), "done")
    assert.equal(closed, true)
  })
})
