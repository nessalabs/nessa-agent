import { describe, expect, it } from "vitest"
import type { WorkerCard } from "./devices"
import { includeRow, moveRow, poolRows, preferenceOf } from "./pool"

const peer = (byte: string) => `peer:${byte.repeat(32)}`
const card = (byte: string, name = byte): WorkerCard =>
  ({ id: byte.repeat(32), environment: peer(byte), name, status: "online" }) as WorkerCard

describe("pool rows", () => {
  it("puts the ordered first, the rest next, the left out last", () => {
    const rows = poolRows([card("aa"), card("bb"), card("cc")], {
      order: [peer("cc")],
      excluded: [peer("aa")],
    })
    expect(rows.map((row) => [row.name, row.included])).toEqual([
      ["cc", true],
      ["bb", true],
      ["aa", false],
    ])
  })

  it("tells two computers with one name apart", () => {
    const rows = poolRows([card("aa", "vm"), card("bb", "vm")], {
      order: [],
      excluded: [],
    })
    expect(rows.map((row) => row.name)).toEqual(["vm · aaaaaa", "vm · bbbbbb"])
  })

  it("saves what is shown and keeps what it names that is not shown", () => {
    const previous = { order: [peer("dd")], excluded: [peer("ee")] }
    let rows = poolRows([card("aa"), card("bb")], previous)
    rows = moveRow(rows, 1, -1)
    rows = includeRow(rows, 1, false)
    expect(preferenceOf(rows, previous)).toEqual({
      order: [peer("bb"), peer("dd")],
      excluded: [peer("aa"), peer("ee")],
    })
  })
})
