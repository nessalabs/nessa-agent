/**
 * Why the source did not do what it was asked, as the workspace holds it: a
 * message marked not sent, an answer to an approval asked again, a
 * conversation or the index that could not be read. State keeps the
 * reason, never a sentence; the words a person is shown for each are the
 * UI's (`ui/failure-copy.ts`).
 *
 * `unavailable`: no answer came — the call may or may not have been done, so
 * a message is sent again under its id and the source's updates say what
 * happened. `unknown-session`: the source holds no such session, archived
 * ones included, and begins none under an archived id. `not-waiting`: the
 * approval was already answered, or never asked. `not-supported`: the source
 * has no such thing to do — a pin, or an answer it does not offer — so it
 * did nothing, and asking again changes nothing.
 */
export type WorkspaceFailureReason =
  "unavailable" | "unknown-session" | "not-waiting" | "not-supported"
