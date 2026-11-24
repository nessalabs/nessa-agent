import type { ComposerNote } from "./composer"

/**
 * What a composer's controls have to say, read aloud and never drawn. A
 * control's note (`ComposerNote`) is drawn as its tooltip and a dot, which a
 * screen reader does not hear arrive: this region, beside the composer and
 * not in it, says each one as it comes. It is the only voice the composer
 * has, and it has no box. Only warnings are said.
 */
export function ComposerSpoken({
  notes,
}: {
  notes: readonly (ComposerNote | undefined)[]
}) {
  // Warnings only: a neutral note is a hint, said again on every chat opened.
  const said = notes.flatMap((note) => (note?.tone === "warning" ? [note.text] : []))
  return (
    <span className="desktop-composer-spoken" role="status" aria-live="polite">
      {said.join(" ")}
    </span>
  )
}
