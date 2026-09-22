import assert from "node:assert/strict"
import { readdirSync } from "node:fs"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

import { adrNumber, duplicateAdrNumbers } from "./adr-numbers.mjs"

const adr = join(dirname(fileURLToPath(import.meta.url)), "../../docs/adr")

test("a four-digit prefix is the number and anything else is not an ADR", () => {
  assert.equal(adrNumber("0014-fetch-agent-runtimes.md"), "0014")
  assert.equal(adrNumber("README.md"), null)
  // The template is not a decision and must not reserve a number.
  assert.equal(adrNumber("template.md"), null)
  // Numbers are padded, so a stray three-digit file is not silently a fifth.
  assert.equal(adrNumber("014-fetch-agent-runtimes.md"), null)
})

test("one number used twice is reported with both files", () => {
  const clashes = duplicateAdrNumbers([
    ["done", ["0013-files-by-path-not-by-payload.md"]],
    ["todo", ["0013-fetch-agent-runtimes.md", "0014-something-else.md"]],
  ])

  assert.deepEqual(clashes, [
    {
      number: "0013",
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
  assert.equal(clashes[0].number, "0005")
})

test("every clash is reported, not the first", () => {
  const clashes = duplicateAdrNumbers([
    ["done", ["0002-a.md", "0003-b.md"]],
    ["todo", ["0002-c.md", "0003-d.md"]],
  ])

  assert.deepEqual(
    clashes.map(({ number }) => number),
    ["0002", "0003"],
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
