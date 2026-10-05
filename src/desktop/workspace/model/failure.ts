/**
 * Why the source did not do what it was asked, as the workspace holds it: a
 * message marked not sent, an answer to an approval asked again, a
 * conversation or the index that could not be read. State keeps the
 * reason, never a sentence; the words a person is shown for each are the
 * UI's (`ui/failure-copy.ts`).
 *
 * `unavailable`: not done now, or not known to be — no answer came, so the
 * call may or may not have been done, or the source answered "not now" and
 * did nothing; either way trying again is right, so a message is sent again
 * under its id and the source's updates say what happened. `unknown-session`: the source holds no such session, archived
 * ones included, and begins none under an archived id. `not-waiting`: the
 * approval was already answered, or never asked. `not-supported`: the source
 * has no such thing to do — a pin, or an answer it does not offer — so it
 * did nothing, and asking again changes nothing. `signed-out`: the window
 * could not connect because the gateway refused its credential — or, in a
 * browser preview, because this origin has no session to present — so the
 * request refused was not sent (a first message's conversation may already
 * have been created before the connection closed); asking again changes
 * nothing until it is signed in again.
 *
 * `not-started`: this window was opened with no local server, so no chat
 * credential exists yet. `not-ready`: the server is still starting.
 * `not-listening`: the server is not answering. `wrong-stage`: this window
 * and the server were asked for different stages; the two values are
 * `StageMismatch`, beside the reason, because a reason cannot carry them.
 */
export type WorkspaceFailureReason =
  | "unavailable"
  | "unknown-session"
  | "not-waiting"
  | "not-supported"
  | "signed-out"
  | "not-started"
  | "not-ready"
  | "not-listening"
  | "wrong-stage"

/** The stage this window is on, and the stage the local server was asked for. */
export type StageMismatch = {
  readonly bundle: string
  readonly requested: string
}
