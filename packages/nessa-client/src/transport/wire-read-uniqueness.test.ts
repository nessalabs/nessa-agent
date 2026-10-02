import { expect, it, vi } from "vitest"
import { WireSession } from "./wire-session.js"
import { createRecordReadApi } from "../presentation/record-read-api.js"
import { createCatalogueReadApi } from "../presentation/catalogue-read-api.js"
import { bounds } from "../generated/product.js"
import { parseWireMessage } from "../protocol/validate.js"

const conversation = "00000000-0000-4000-8000-000000000001"
const scope = {
  receiver: "receiver",
  origin: "gateway",
  stream: conversation,
  incarnation: "incarnation",
  schema: "schema",
  accessEpoch: "epoch-1",
}
const request = {
  scope,
  after: "0",
  target: "1",
  maxRecords: 1,
  maxPayloadBytes: bounds.maxRecordPagePayloadBytes,
  maxRecordBytes: bounds.maxPhysicalRecordPayloadBytes,
}
const pass = { scope, completed: "0", boundary: "1", generation: "1" }
const manifest = { pass, maxEntries: 1 }
const descriptor = {
  key: { creation: "1", id: "conversation" },
  revision: "1",
  deleted: true,
}

it("rejects raw duplicates through actual read APIs and preserves unique foreign evidence", async () => {
  vi.stubGlobal("WebSocket", { OPEN: 1 })
  try {
    let deliver: (event: { data: string }) => void = () => {}
    let correlation = ""
    const close = vi.fn()
    const socket = {
      readyState: 1,
      close,
      send(raw: string) {
        correlation = JSON.parse(raw).id
      },
      addEventListener(name: string, listener: (event: { data: string }) => void) {
        if (name === "message") deliver = listener
      },
    } as unknown as WebSocket
    const session = new WireSession(socket, { requestTimeoutMs: 1000 })
    const records = createRecordReadApi(session)
    const catalogue = createCatalogueReadApi(session)
    const cases = [
      {
        call: () => records.head(conversation, "receiver", "1"),
        payload: { scope, head: "1" },
      },
      {
        call: () => records.page(conversation, "1", request),
        payload: { request, records: [{ position: "1", id: "record", payload: "AQ==" }] },
      },
      {
        call: () => catalogue.head({ receiverId: "receiver", accessEpoch: "1" }),
        payload: { scope, head: "1" },
      },
      {
        call: () => catalogue.manifest({ request: manifest, accessEpoch: "1" }),
        payload: { request: manifest, entries: [descriptor], hasMore: false },
      },
      {
        call: () =>
          catalogue.resolve({ pass, descriptor, accessEpoch: "1", maxPayloadBytes: 1 }),
        payload: { pass, descriptor, entry: descriptor, payload: "" },
      },
    ]
    for (const sample of cases) {
      const pending = sample.call()
      const settled = vi.fn()
      void pending.then(settled)
      const raw = JSON.stringify({
        type: "res",
        id: correlation,
        ok: true,
        payload: sample.payload,
      })
      for (const malformed of [
        raw + " trailing",
        raw.replace('"receiver":"receiver"', '"receiver":"\\x41"'),
        raw.slice(0, -1),
      ]) {
        deliver({ data: malformed })
        await Promise.resolve()
        expect(settled).not.toHaveBeenCalled()
      }
      for (const field of ["receiver", "incarnation", "accessEpoch"]) {
        const value = scope[field as keyof typeof scope]
        for (const values of [
          ["foreign", value],
          [value, "foreign"],
          [value, value],
        ]) {
          const key = JSON.stringify(field)
          const ambiguous = raw.replace(
            `${key}:${JSON.stringify(value)}`,
            `${key}:${JSON.stringify(values[0])},${key}:${JSON.stringify(values[1])}`,
          )
          deliver({ data: ambiguous })
          await Promise.resolve()
          expect(settled).not.toHaveBeenCalled()
        }
        const escaped = JSON.stringify(field).replace(
          field[1],
          `\\u${field.charCodeAt(1).toString(16).padStart(4, "0")}`,
        )
        deliver({
          data: raw.replace(
            `"${field}":${JSON.stringify(value)}`,
            `"${field}":${JSON.stringify(value)},${escaped}:${JSON.stringify(value)}`,
          ),
        })
        await Promise.resolve()
        expect(settled).not.toHaveBeenCalled()
      }
      // Exercise nested echo/descriptor/record identities through the APIs too.
      for (const token of JSON.stringify(sample.payload).matchAll(
        /"(?:\\.|[^"\\])*":"(?:\\.|[^"\\])*"/g,
      )) {
        const [key, value] = token[0].split(":")
        for (const duplicate of [
          `${key}:"foreign",${key}:${value}`,
          `${key}:${value},${key}:"foreign"`,
          `${key}:${value},${key}:${value}`,
        ]) {
          deliver({ data: raw.replace(token[0], duplicate) })
          await Promise.resolve()
          expect(settled).not.toHaveBeenCalled()
        }
      }
      for (const container of ["scope", "request", "pass", "descriptor", "key"]) {
        if (!raw.includes(`"${container}":`)) continue
        deliver({
          data: raw.replace(`"${container}":`, `"${container}":null,"${container}":`),
        })
        await Promise.resolve()
        expect(settled).not.toHaveBeenCalled()
      }
      // Correlation itself must not collapse before the session sees it.
      deliver({
        data: raw.replace(
          `"id":"${correlation}"`,
          `"id":"foreign","id":"${correlation}"`,
        ),
      })
      await Promise.resolve()
      expect(settled).not.toHaveBeenCalled()
      // The API deliberately preserves one unambiguous foreign scope for core.
      deliver({ data: raw.replace('"receiver":"receiver"', '"receiver":"foreign"') })
      await pending
      expect(settled).toHaveBeenCalledOnce()
    }
    const received = vi.fn()
    session.onEvent("custom", received)
    for (const value of ["1e2", "-0", "1e400", '"\\ud800"', '"quote\\" braces{}[] 😀"']) {
      const raw = `{"type":"event","event":"custom","seq":0,"stateVersion":0,"payload":${value}}`
      deliver({ data: raw })
      expect(received).toHaveBeenLastCalledWith(JSON.parse(raw).payload)
    }
    expect(close).not.toHaveBeenCalled()
    session.close()
  } finally {
    vi.unstubAllGlobals()
  }
})

it("retains JSON grammar, number and Unicode conversion with unique object keys", () => {
  const payload = {
    number: -0,
    text: 'quote" slash\\ braces{}[] 😀',
    nested: [{ key: "one" }, { key: "two" }],
  }
  const raw = JSON.stringify({
    type: "event",
    event: "custom",
    seq: 0,
    stateVersion: 0,
    payload,
  })
  expect(parseWireMessage(raw)).toEqual(JSON.parse(raw))
  for (const value of ["1e2", "-0", "1.5", "1e400", '"\\ud800"', '"\\u0065"']) {
    const text = `{"type":"event","event":"custom","seq":0,"stateVersion":0,"payload":${value}}`
    expect(parseWireMessage(text)).toEqual(JSON.parse(text))
  }
  for (const raw of [
    '{"type":"event",}',
    '{"payload":01}',
    '{"payload":"\\x41"}',
    '{"payload":"unterminated}',
    "[] trailing",
    '{"payload":[1,]}',
  ]) {
    expect(parseWireMessage(raw)).toBeNull()
  }
  for (const payload of [
    '{"key":"a","key":"b"}',
    '{"key":"a","k\\u0065y":"a"}',
    '{"items":[{"id":"a","id":"b"}]}',
  ]) {
    expect(
      parseWireMessage(
        `{"type":"event","event":"custom","seq":0,"stateVersion":0,"payload":${payload}}`,
      ),
    ).toBeNull()
  }
})
