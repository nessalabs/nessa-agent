/** Checks only raw object-key uniqueness. JSON.parse owns syntax and values.
 * Sets are local to each object, so equal keys in different objects are valid.
 */
export function hasUniqueObjectKeys(raw: string): boolean {
  const objects: (Set<string> | null)[] = []
  for (let index = 0; index < raw.length; index++) {
    const character = raw[index]
    if (character === "{") objects.push(new Set())
    else if (character === "[") objects.push(null)
    else if (character === "}" || character === "]") objects.pop()
    else if (character === '"') {
      const start = index
      index++
      while (index < raw.length && raw[index] !== '"') {
        if (raw[index] === "\\") index++
        index++
      }
      let following = index + 1
      while (/^[\t\n\r ]$/.test(raw[following] ?? "")) following++
      const keys = objects.at(-1)
      if (keys && raw[following] === ":") {
        // Decode spelling only: escaped key aliases must share one identity.
        const key: string = JSON.parse(raw.slice(start, index + 1))
        if (keys.has(key)) return false
        keys.add(key)
      }
    }
  }
  return true
}
