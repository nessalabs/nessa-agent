import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

test("the embedded document explains a frontend that never replaces it", () => {
  const document = readFileSync("index.html", "utf8")

  assert.match(document, /data-nessa-load-fallback/)
  assert.match(document, /Loading Nessa/)
  assert.match(document, /If this stays on screen/)
})
