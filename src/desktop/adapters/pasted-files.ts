/**
 * The files a paste carries, when it carries no text.
 *
 * A copied screenshot arrives as a file and nothing else; WebKit hands it over
 * in `files`, and another engine may list it only among the `items`, so both
 * are read, `files` first. A paste with text in it is text: a spreadsheet's
 * cells also bring a picture of themselves, and the cells are what was meant.
 * Answers no files for such a paste, which the field then takes as it would.
 */
export function pastedFiles(data: DataTransfer | null): File[] {
  if (!data || data.getData("text/plain")) return []
  const files = Array.from(data.files ?? [])
  if (files.length) return files
  return Array.from(data.items ?? []).flatMap((item) => {
    const file = item.kind === "file" ? item.getAsFile() : null
    return file ? [file] : []
  })
}
