import fs from "node:fs"
import crypto from "node:crypto"
const { default: MarkdownIt } =
  await import("/workspace/nessa-agent/node_modules/.pnpm/markdown-it@14.3.1/node_modules/markdown-it/index.mjs")
const renderer = new MarkdownIt()
const paths = {
  before:
    "/tmp/627-consolidated-review-checkpoint/source/docs/design/bounded-physical-persistence.md",
  after:
    "/workspace/nessa-agent-bounded-persistence/docs/design/bounded-physical-persistence.md",
}
const proof = { renderer: "markdown-it14.3.1", results: {} }
for (const [label, path] of Object.entries(paths)) {
  const source = fs.readFileSync(path, "utf8")
  const tokens = renderer.parse(source, {})
  const tables = []
  let current = null
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]
    if (token.type === "table_open") {
      current = []
      tables.push(current)
    }
    if (token.type === "table_close") current = null
    if (current && token.type === "tr_open" && tokens[i + 1]?.type === "td_open") {
      const content = tokens[i + 2]?.content
      if (/^\d+$/.test(content ?? "")) current.push(Number(content))
    }
  }
  const sourceRows = [...source.matchAll(/^\|\s*(\d+)\s*\|/gm)].map((x) => Number(x[1]))
  const html = renderer.render(source)
  fs.writeFileSync(`/tmp/627-consolidated-table-render-${label}.html`, html)
  proof.results[label] = {
    source_path: path,
    source_sha256: crypto.createHash("sha256").update(source).digest("hex"),
    source_rows: sourceRows,
    rendered_table_rows: tables,
  }
}
if (
  JSON.stringify(proof.results.after.rendered_table_rows) !==
  JSON.stringify([Array.from({ length: 26 }, (_, i) => i + 1)])
)
  throw Error("Canonical cases did not render as one26-row table")
fs.writeFileSync(
  "/tmp/627-consolidated-table-render-proof.json",
  JSON.stringify(proof, null, 2) + "\n",
)
console.log(JSON.stringify(proof, null, 2))
