import assert from "node:assert/strict"
import { readdirSync } from "node:fs"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { adrNumber, duplicateAdrNumbers } from "./adr-numbers.mjs"

const adr = join(dirname(fileURLToPath(import.meta.url)), "../../docs/adr")

test("the leading digits are the number, however many there are", () => {
  // The old era, padded to four.
  assert.equal(adrNumber("0014-nessa-owned-policy-hooks.md"), "0014")
  // The new one: the number of the issue that proposed it, as issues spell it.
  assert.equal(adrNumber("142-some-decision.md"), "142")
  assert.equal(adrNumber("README.md"), null)
  assert.equal(adrNumber("template.md"), null)
})

test("padding is spelling, not identity", () => {
  // `0014` and `14` are one number written two ways, and a record that took the
  // second while another holds the first is the clash this exists to catch —
  // not two records that merely look different in a directory listing.
  const clashes = duplicateAdrNumbers([
    ["done", ["0014-nessa-owned-policy-hooks.md"]],
    ["todo", ["14-something-else.md"]],
  ])

  assert.equal(clashes.length, 1)
  assert.equal(clashes[0].number, "14")
})

test("an issue-numbered record does not collide with the old block", () => {
  // The whole reason the scheme works: issues are past 140 and the records
  // written before the rule stop at 0014, so the ranges cannot meet.
  const clashes = duplicateAdrNumbers([
    ["done", ["0013-files-by-path-not-by-payload.md"]],
    ["todo", ["0014-nessa-owned-policy-hooks.md", "173-a-later-decision.md"]],
  ])

  assert.deepEqual(clashes, [])
})

test("one number used twice is reported with both files", () => {
  const clashes = duplicateAdrNumbers([
    ["done", ["0013-files-by-path-not-by-payload.md"]],
    ["todo", ["0013-fetch-agent-runtimes.md", "0014-something-else.md"]],
  ])

  assert.deepEqual(clashes, [
    {
      // The identity, not the spelling — the paths below carry that.
      number: "13",
      paths: [
        "done/0013-files-by-path-not-by-payload.md",
        "todo/0013-fetch-agent-runtimes.md",
      ],
    },
  ])
})

test("todo and done share one sequence, so a number survives being implemented", () => {
  // The real shape of the collision that got through twice: the number is free
  // in the directory the author is looking at and taken in the other one.
  const clashes = duplicateAdrNumbers([
    ["done", ["0005-stage-scoped-local-data.md"]],
    ["todo", ["0005-stage-scoped-local-data.md"]],
  ])

  assert.equal(clashes.length, 1)
  assert.equal(clashes[0].number, "5")
})

test("every clash is reported, not the first", () => {
  const clashes = duplicateAdrNumbers([
    ["done", ["0002-a.md", "0003-b.md"]],
    ["todo", ["0002-c.md", "0003-d.md"]],
  ])

  assert.deepEqual(
    clashes.map(({ number }) => number),
    ["2", "3"],
  )
})

test("the records on disk use each number once", () => {
  const entries = ["done", "todo"].map((directory) => [
    directory,
    readdirSync(join(adr, directory)),
  ])

  const clashes = duplicateAdrNumbers(entries)

  assert.deepEqual(
    clashes,
    [],
    clashes
      .map(({ number, paths }) => `ADR ${number} is used by ${paths.join(" and ")}`)
      .join("; "),
  )
})
