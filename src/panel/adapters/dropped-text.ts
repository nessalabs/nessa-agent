/** Browser links expose URI lists; ignore their comment lines and retain all URLs. */
export function droppedText(data: Pick<DataTransfer, "getData">): string {
  const plain = data.getData("text/plain")
  const html = data.getData("text/html")
  if (html) {
    const document = new DOMParser().parseFromString(html, "text/html")
    // A rich text selection may advertise its inline image as a URI. Keep the
    // selected prose instead; a standalone link still uses its actual URL below.
    if (document.querySelector("img") && document.body.textContent?.trim())
      return plain || document.body.textContent
  }
  const urls = data
    .getData("text/uri-list")
    .split(/\r?\n/)
    .filter((line) => line.trim() && !line.startsWith("#"))
    .join("\n")
  return urls || plain
}
