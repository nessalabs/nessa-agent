/**
 * What an app may say, read from the spec's own message examples (MCP Apps
 * 2026-01-26, *MCP Apps Specific Messages*, *Display Modes*, *Transport
 * Layer*), and refused when it says something else (#349, L28).
 */
import { describe, expect, it } from "vitest"
import { errorCodes, readEnvelope } from "./json-rpc"
import { contentText, readFromFrame } from "./messages"

const read = (data: unknown) => readFromFrame(readEnvelope(data))

describe("the spec's requests", () => {
  it("ui/initialize, with the display modes the view declares", () => {
    expect(
      read({
        jsonrpc: "2.0",
        id: 1,
        method: "ui/initialize",
        params: {
          appInfo: { name: "My UI", version: "1.0.0" },
          appCapabilities: { availableDisplayModes: ["inline", "fullscreen"] },
          protocolVersion: "2026-01-26",
        },
      }),
    ).toEqual({
      kind: "request",
      id: 1,
      request: {
        method: "ui/initialize",
        initialize: {
          protocolVersion: "2026-01-26",
          appName: "My UI",
          displayModes: ["inline", "fullscreen"],
        },
      },
    })
  })

  it("ui/open-link, ui/message, ui/request-display-mode, ui/update-model-context", () => {
    expect(
      read({
        jsonrpc: "2.0",
        id: 1,
        method: "ui/open-link",
        params: { url: "https://a.example/" },
      }),
    ).toEqual({
      kind: "request",
      id: 1,
      request: { method: "ui/open-link", url: "https://a.example/" },
    })
    expect(
      read({
        jsonrpc: "2.0",
        id: 2,
        method: "ui/message",
        params: { role: "user", content: [{ type: "text", text: "Hello" }] },
      }),
    ).toEqual({
      kind: "request",
      id: 2,
      request: { method: "ui/message", content: [{ type: "text", text: "Hello" }] },
    })
    expect(
      read({
        jsonrpc: "2.0",
        id: 3,
        method: "ui/request-display-mode",
        params: { mode: "fullscreen" },
      }),
    ).toEqual({
      kind: "request",
      id: 3,
      request: { method: "ui/request-display-mode", mode: "fullscreen" },
    })
    expect(
      read({
        jsonrpc: "2.0",
        id: 3,
        method: "ui/update-model-context",
        params: { structuredContent: { selected: 2 } },
      }),
    ).toEqual({
      kind: "request",
      id: 3,
      request: { method: "ui/update-model-context", structuredContent: { selected: 2 } },
    })
  })

  it("tools/call and resources/read, the arguments a copy of their own", () => {
    const params = { name: "get_weather", arguments: { location: "New York" } }
    const message = read({ jsonrpc: "2.0", id: 4, method: "tools/call", params })
    params.arguments.location = "changed"
    expect(message).toEqual({
      kind: "request",
      id: 4,
      request: {
        method: "tools/call",
        tool: "get_weather",
        arguments: { location: "New York" },
      },
    })
    expect(
      read({
        jsonrpc: "2.0",
        id: 5,
        method: "resources/read",
        params: { uri: "ui://w/x" },
      }),
    ).toEqual({
      kind: "request",
      id: 5,
      request: { method: "resources/read", uri: "ui://w/x" },
    })
  })

  it("ui/download-file, embedded resources only", () => {
    expect(
      read({
        jsonrpc: "2.0",
        id: 6,
        method: "ui/download-file",
        params: {
          contents: [
            {
              type: "resource",
              resource: { uri: "file:///report.csv", mimeType: "text/csv", text: "a,b" },
            },
          ],
        },
      }),
    ).toEqual({
      kind: "request",
      id: 6,
      request: {
        method: "ui/download-file",
        contents: [
          {
            uri: "file:///report.csv",
            mimeType: "text/csv",
            content: { kind: "text", text: "a,b" },
          },
        ],
      },
    })
    expect(
      read({
        jsonrpc: "2.0",
        id: 7,
        method: "ui/download-file",
        params: { contents: [{ type: "resource_link", uri: "https://a.example/f" }] },
      }),
    ).toMatchObject({ kind: "refused", id: 7, code: errorCodes.invalidParams })
  })
})

describe("the spec's notifications", () => {
  it("initialized, size-changed, request-teardown, and the proxy's", () => {
    expect(read({ jsonrpc: "2.0", method: "ui/notifications/initialized" })).toEqual({
      kind: "notification",
      notification: { method: "ui/notifications/initialized" },
    })
    expect(
      read({
        jsonrpc: "2.0",
        method: "ui/notifications/size-changed",
        params: { width: 400, height: 600 },
      }),
    ).toEqual({
      kind: "notification",
      notification: { method: "ui/notifications/size-changed", width: 400, height: 600 },
    })
    expect(read({ jsonrpc: "2.0", method: "ui/notifications/request-teardown" })).toEqual(
      {
        kind: "notification",
        notification: { method: "ui/notifications/request-teardown" },
      },
    )
    expect(
      read({
        jsonrpc: "2.0",
        method: "ui/notifications/sandbox-proxy-ready",
        params: {},
      }),
    ).toEqual({
      kind: "notification",
      notification: { method: "ui/notifications/sandbox-proxy-ready" },
    })
    expect(
      read({
        jsonrpc: "2.0",
        method: "ui/notifications/sandbox-csp-violation",
        params: { origin: "https://example.com" },
      }),
    ).toEqual({
      kind: "notification",
      notification: {
        method: "ui/notifications/sandbox-csp-violation",
        origin: "https://example.com",
      },
    })
  })

  it("a response to the host's request is an answer, by its id", () => {
    expect(read({ jsonrpc: "2.0", id: "nessa-teardown", result: {} })).toEqual({
      kind: "answer",
      id: "nessa-teardown",
    })
  })
})

describe("what a report and a message may carry", () => {
  const report = (origin: unknown) =>
    read({
      jsonrpc: "2.0",
      method: "ui/notifications/sandbox-csp-violation",
      params: { origin },
    })

  it("a blocked origin only when it is a web origin as the URL parser writes it", () => {
    for (const origin of [
      "https://example.com",
      "http://127.0.0.1:8080",
      "wss://rt.example",
    ])
      expect(report(origin), origin).toEqual({
        kind: "notification",
        notification: { method: "ui/notifications/sandbox-csp-violation", origin },
      })
    for (const origin of [
      "Your session expired - sign in at https://evil.example",
      "https://example.com/",
      "https://example.com/path",
      "HTTPS://EXAMPLE.COM",
      "javascript:alert(1)",
      "file:///etc",
      "null",
      7,
    ])
      expect(report(origin), String(origin)).toEqual({
        kind: "notification",
        notification: { method: "ui/notifications/sandbox-csp-violation" },
      })
  })

  it("text blocks only, rebuilt from their text alone", () => {
    expect(
      read({
        jsonrpc: "2.0",
        id: 1,
        method: "ui/message",
        params: {
          role: "user",
          content: [{ type: "text", text: "Hi", extra: { a: 1 } }],
        },
      }),
    ).toEqual({
      kind: "request",
      id: 1,
      request: { method: "ui/message", content: [{ type: "text", text: "Hi" }] },
    })
    for (const block of [
      { type: "image", data: "AAAA", mimeType: "image/png" },
      // Another modality that also carries text: refused by its type.
      { type: "image", text: "caption", data: "AAAA", mimeType: "image/png" },
      { type: "resource_link", uri: "https://a.example" },
      { type: "text" },
      { type: "text", text: 3 },
    ])
      expect(
        read({
          jsonrpc: "2.0",
          id: 2,
          method: "ui/update-model-context",
          params: { content: [block] },
        }),
        block.type,
      ).toMatchObject({ kind: "refused", code: errorCodes.invalidParams })
  })
})

describe("what the host refuses or ignores", () => {
  it("a request's params of the wrong shape are invalid params, under its id", () => {
    for (const [method, params] of [
      ["ui/initialize", { appInfo: { name: "x" }, protocolVersion: "2026-01-26" }],
      ["ui/initialize", { appInfo: {}, appCapabilities: {}, protocolVersion: "v" }],
      [
        "ui/initialize",
        {
          appInfo: { name: "x" },
          appCapabilities: { availableDisplayModes: ["sideways"] },
          protocolVersion: "v",
        },
      ],
      ["tools/call", {}],
      ["tools/call", { name: "" }],
      ["tools/call", { name: "t", arguments: [1] }],
      ["resources/read", { uri: 3 }],
      ["ui/message", { role: "assistant", content: [{ type: "text", text: "x" }] }],
      ["ui/message", { role: "user", content: { type: "text", text: "x" } }],
      ["ui/message", { role: "user", content: [] }],
      ["ui/message", { role: "user", content: [{ text: "no type" }] }],
      ["ui/message", { role: "user", content: [{ type: "image", data: "AAAA" }] }],
      ["ui/update-model-context", { content: "x" }],
      ["ui/update-model-context", { structuredContent: [1] }],
      ["ui/open-link", {}],
      ["ui/request-display-mode", { mode: "sideways" }],
      ["ui/download-file", { contents: [] }],
    ] as const)
      expect(read({ jsonrpc: "2.0", id: 9, method, params }), method).toEqual({
        kind: "refused",
        id: 9,
        code: errorCodes.invalidParams,
        message: "Invalid params",
      })
  })

  it("an unknown request is not found; an unknown notification is ignored", () => {
    expect(
      read({ jsonrpc: "2.0", id: 1, method: "sampling/createMessage" }),
    ).toMatchObject({
      kind: "refused",
      id: 1,
      code: errorCodes.methodNotFound,
    })
    expect(read({ jsonrpc: "2.0", id: 1, method: "toString" })).toMatchObject({
      kind: "refused",
      code: errorCodes.methodNotFound,
    })
    expect(read({ jsonrpc: "2.0", method: "ui/notifications/whatever" })).toEqual({
      kind: "ignored",
    })
  })

  it("a malformed message with an id is an invalid request; without one, ignored", () => {
    expect(read({ jsonrpc: "2.0", id: 3, method: "ping", params: [] })).toEqual({
      kind: "refused",
      id: 3,
      code: errorCodes.invalidRequest,
      message: "Invalid request",
    })
    expect(read("hello")).toEqual({ kind: "ignored" })
  })

  it("a size it cannot read is ignored", () => {
    for (const params of [
      { width: -1 },
      { height: "10" },
      { height: Number.MAX_VALUE * 2 },
    ])
      expect(
        read({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params }),
      ).toEqual({ kind: "ignored" })
  })
})

describe("what an app's blocks say (#390, D-A)", () => {
  it("D-A: is each block's text, a blank line between each, an empty block left out", () => {
    const text = (value: string) => ({ type: "text", text: value })
    expect(contentText([text("Plot May"), text("next to April")])).toBe(
      "Plot May\n\nnext to April",
    )
    expect(contentText([text(""), text("a"), text(""), text("b"), text("")])).toBe(
      "a\n\nb",
    )
    // Blank but not empty is kept as given: whether it says anything is the gateway's (M3).
    expect(contentText([text(" "), text("\n")])).toBe(" \n\n\n")
    expect(contentText([])).toBe("")
    expect(contentText([text(""), text("")])).toBe("")
  })
})
