/** The loading avatar, with its blur and grain left off, for the startup screen. */

export function flatStartupMark(svg) {
  return svg.replaceAll(/ filter="url\(#[^"]*\)"/g, "")
}
