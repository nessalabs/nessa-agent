import assert from "node:assert/strict"
import test from "node:test"
import { answer, PIN_URI, TOOL_NAME } from "./panel-pin.mjs"

const call = (method, params) =>
  answer({ jsonrpc: "2.0", id: 1, method, ...(params ? { params } : {}) })

test("panel_pin is one fullscreen app and nothing else", () => {
  const listed = call("tools/list")
  assert.equal(listed.result.tools.length, 1)
  assert.equal(listed.result.tools[0].name, TOOL_NAME)
  assert.equal(listed.result.tools[0]._meta.ui.resourceUri, PIN_URI)
  const read = call("resources/read", { uri: PIN_URI })
  const html = read.result.contents[0].text
  assert.match(html, /data-pin="panel"/)
  assert.match(html, /"inline", "fullscreen"/)
  assert.equal(call("tools/call", { name: TOOL_NAME }).result.isError, undefined)
  assert.equal(call("tools/call", { name: "talk_board" }).error.code, -32602)
})
