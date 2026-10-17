/**
 * What an agent's command touches, read from its words so the card can say
 * it at a glance (`approval-card.tsx`): the programs it runs and the sites it
 * reaches. The command itself stays the authority; the gist only points at
 * it, and the card's Details show it whole.
 *
 * A command arrives as the agent's shell words (`cargo test -p gateway`) or
 * as a tool and its JSON input (`mcp__nessa__shell {"command":"curl …"}`).
 * A tool whose input carries a shell script — a `command` string, or a
 * `command` list such as `["bash", "-lc", "…"]` — is read as that script;
 * any other tool is only its name. The reading is a sketch, not a shell: it
 * knows quotes, escapes, and the operators that start a new program (`;`,
 * `&&`, `||`, `|`, `&`, a newline, a subshell), and no more.
 */

/** A program the command runs, and how many times. */
export interface GistProgram {
  readonly name: string
  readonly count: number
}

export interface CommandGist {
  /** The tool the command calls, when it is a tool and its input. */
  readonly tool?: string
  /** The shell script it runs, when there is one, one step per line (`scriptLines`). */
  readonly script?: string
  /** The input's other fields, shown beside the script: `[name, JSON value]`. */
  readonly fields: readonly (readonly [string, string])[]
  /** The programs, in the order they first run. */
  readonly programs: readonly GistProgram[]
  /** The sites reached, by host, in the order they first appear. */
  readonly hosts: readonly string[]
}

const shells = new Set(["bash", "sh", "zsh", "dash", "fish"])

/** A shell list such as `["bash", "-lc", "…"]` is its script; any other list, its words. */
function scriptOf(value: unknown): string | undefined {
  if (typeof value === "string") return value
  if (!Array.isArray(value) || !value.every((word) => typeof word === "string"))
    return undefined
  const words = value as string[]
  const flag = words[1]
  if (
    words.length === 3 &&
    shells.has(words[0]?.split("/").at(-1) ?? "") &&
    flag !== undefined &&
    /^-[a-z]*c[a-z]*$/.test(flag)
  )
    return words[2]
  return words.join(" ")
}

/** The tool and its JSON input, when the command is that; otherwise nothing. */
function toolCall(
  command: string,
): { tool: string; input: Record<string, unknown> } | undefined {
  const space = command.indexOf(" ")
  if (space <= 0) return undefined
  const rest = command.slice(space + 1).trim()
  if (!rest.startsWith("{")) return undefined
  try {
    const input = JSON.parse(rest) as unknown
    if (input === null || typeof input !== "object" || Array.isArray(input))
      return undefined
    return { tool: command.slice(0, space), input: input as Record<string, unknown> }
  } catch {
    return undefined
  }
}

interface Word {
  readonly text: string
  /** It began a new program: the first word after an operator. */
  readonly starts: boolean
}

/** The script's words, unquoted, each marked where a new program starts. */
function words(script: string): Word[] {
  const out: Word[] = []
  let text = ""
  let inWord = false
  let starts = true
  let quote: "'" | '"' | undefined
  const end = () => {
    if (inWord) out.push({ text, starts })
    if (inWord) starts = false
    text = ""
    inWord = false
  }
  for (let index = 0; index < script.length; index++) {
    const char = script[index]
    if (quote) {
      if (char === quote) quote = undefined
      else if (char === "\\" && quote === '"' && index + 1 < script.length)
        text += script[++index]
      else text += char
      continue
    }
    if (char === "'" || char === '"') {
      quote = char
      inWord = true
      continue
    }
    if (char === "\\" && index + 1 < script.length) {
      text += script[++index]
      inWord = true
      continue
    }
    // `2>&1` and `&>` redirect; they do not start a program.
    const redirect = char === "&" && (/[<>]$/.test(text) || script[index + 1] === ">")
    if (";&|\n(".includes(char) && !redirect) {
      end()
      starts = true
      continue
    }
    if (char === ")") {
      end()
      continue
    }
    if (char === " " || char === "\t") {
      end()
      continue
    }
    text += char
    inWord = true
  }
  end()
  return out
}

/** `NAME=value` before a program sets its environment; it is not the program. */
const assignment = /^[A-Za-z_][A-Za-z0-9_]*=/

/** The host of a web address in a word, lowercased; nothing when it names none. */
function hostIn(word: string): string | undefined {
  const match = /^(?:https?|wss?|ftp):\/\/([^/?#\s]+)/i.exec(word)
  if (!match) return undefined
  const authority = match[1] ?? ""
  const host = authority.slice(authority.lastIndexOf("@") + 1).replace(/:\d+$/, "")
  return host.length > 0 ? host.toLowerCase() : undefined
}

/**
 * The script with one step per line: a new line after each `;`, `&&` and
 * `||` outside quotes, and each `|` beginning an indented line under the
 * step it feeds. A script that already has lines keeps them.
 */
export function scriptLines(script: string): string {
  let out = ""
  let quote: "'" | '"' | undefined
  for (let index = 0; index < script.length; index++) {
    const char = script[index]
    const next = script[index + 1]
    if (quote) {
      out += char
      if (char === "\\" && quote === '"' && next !== undefined) out += script[++index]
      else if (char === quote) quote = undefined
      continue
    }
    if (char === "'" || char === '"') quote = char
    if (char === "\\" && next !== undefined) {
      out += char + script[++index]
      continue
    }
    if (char === ";" && next !== ";") {
      out = `${out.trimEnd()};\n`
      while (script[index + 1] === " ") index++
      continue
    }
    if ((char === "&" && next === "&") || (char === "|" && next === "|")) {
      out = `${out.trimEnd()}\n${char}${next} `
      index++
      while (script[index + 1] === " ") index++
      continue
    }
    if (char === "|") {
      out = `${out.trimEnd()}\n  | `
      while (script[index + 1] === " ") index++
      continue
    }
    out += char
  }
  return out.trim()
}

/** What the command runs and reaches (`CommandGist`). */
export function commandGist(command: string): CommandGist {
  const call = toolCall(command)
  const script = call ? scriptOf(call.input.command) : command
  const fields = call
    ? Object.entries(call.input)
        .filter(([name]) => script === undefined || name !== "command")
        .map(([name, value]) => [name, JSON.stringify(value)] as const)
    : []
  const programs = new Map<string, number>()
  const hosts: string[] = []
  let program = true
  for (const word of script === undefined ? [] : words(script)) {
    if (word.starts) program = true
    if (program && !assignment.test(word.text)) {
      programs.set(word.text, (programs.get(word.text) ?? 0) + 1)
      program = false
    }
    const host = hostIn(word.text)
    if (host && !hosts.includes(host)) hosts.push(host)
  }
  return {
    ...(call ? { tool: call.tool } : {}),
    ...(script !== undefined ? { script: scriptLines(script) } : {}),
    fields,
    programs: [...programs].map(([name, count]) => ({ name, count })),
    hosts,
  }
}
