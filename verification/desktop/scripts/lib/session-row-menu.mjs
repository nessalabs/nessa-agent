/** The row's product menu, including Fork; sharing appears only when available. */
export function sessionRowMenuMatches(words) {
  const expected = ["Rename", words[1] === "Unpin" ? "Unpin" : "Pin", "Fork"]
  if (words.includes("Share…")) expected.push("Share…")
  expected.push("Archive", "Delete…")
  return (
    words.length === expected.length && words.every((word, i) => word === expected[i])
  )
}
