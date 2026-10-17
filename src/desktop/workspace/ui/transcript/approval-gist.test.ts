/**
 * What an agent's command touches, read from its words (`approval-gist.ts`):
 * the programs it runs and the sites it reaches, and its script one step per
 * line, for every shape a command arrives in.
 */
import { expect, it } from "vitest"
import { commandGist, scriptLines } from "./approval-gist"

const shell = String.raw`curl -s -A "Mozilla/5.0" "https://tad2026.eventive.org/api/v1/films" | head -c 1000; echo; echo "---"; curl -s -I "https://tad2026.eventive.org/api/v1/films"; curl -s https://tad2026.eventive.org/schedule | grep -oE 'src="[^"]+\.js"' | head -20`

it("reads a shell tool's script: its programs, counted, and its sites", () => {
  const gist = commandGist(`mcp__nessa__shell ${JSON.stringify({ command: shell })}`)
  expect(gist.tool).toBe("mcp__nessa__shell")
  expect(gist.programs).toEqual([
    { name: "curl", count: 3 },
    { name: "head", count: 2 },
    { name: "echo", count: 2 },
    { name: "grep", count: 1 },
  ])
  expect(gist.hosts).toEqual(["tad2026.eventive.org"])
  expect(gist.fields).toEqual([])
})

it("puts each step of a script on its own line, a pipe under the step it feeds", () => {
  expect(scriptLines("a | b -c 'x | y'; echo \"1; 2\" && c || d")).toBe(
    "a\n  | b -c 'x | y';\necho \"1; 2\"\n&& c\n|| d",
  )
})

it("reads an agent's plain shell words as the script, with no tool", () => {
  const gist = commandGist(
    "RUST_LOG=debug cargo test -p nessa-gateway -- --nocapture 2>&1",
  )
  expect(gist.tool).toBeUndefined()
  expect(gist.programs).toEqual([{ name: "cargo", count: 1 }])
  expect(gist.script).toBe(
    "RUST_LOG=debug cargo test -p nessa-gateway -- --nocapture 2>&1",
  )
})

it("reads a shell list as its script, and keeps the input's other fields", () => {
  const gist = commandGist(
    `exec ${JSON.stringify({ command: ["/bin/bash", "-lc", "git push origin main"], cwd: "/repo" })}`,
  )
  expect(gist.programs).toEqual([{ name: "git", count: 1 }])
  expect(gist.script).toBe("git push origin main")
  expect(gist.fields).toEqual([["cwd", '"/repo"']])
})

it("names only the tool when its input carries no script", () => {
  const gist = commandGist(
    `Write ${JSON.stringify({ file_path: "a.txt", content: "hi" })}`,
  )
  expect(gist.tool).toBe("Write")
  expect(gist.script).toBeUndefined()
  expect(gist.programs).toEqual([])
  expect(gist.fields).toHaveLength(2)
})

it("finds a site past its credentials and port, and counts a subshell's program", () => {
  const gist = commandGist(
    "echo $(date) done; git clone https://me:token@Example.COM:8443/r.git && open http://localhost:3000",
  )
  expect(gist.hosts).toEqual(["example.com", "localhost"])
  expect(gist.programs.map((program) => program.name)).toEqual([
    "echo",
    "date",
    "git",
    "open",
  ])
})
