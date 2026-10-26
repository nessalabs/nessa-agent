/**
 * The GitHub sign-in in Settings' words: signed out (and why the last try
 * ended), waiting for a code to be entered, or signed in as someone. The
 * gateway decides each; this only names them. The sentences for why a try
 * ended belong to the adapter, which reads the gateway's codes.
 */
export type GithubView =
  | { readonly kind: "signedOut"; readonly ended?: string; readonly installUrl: string }
  | {
      readonly kind: "connecting"
      readonly userCode: string
      readonly verificationUri: string
      /** How long the code has left, as of this answer. */
      readonly expiresInSeconds: number
      readonly installUrl: string
    }
  | {
      readonly kind: "connected"
      /** GitHub's numeric id for the account, which never changes: what its picture is found by. */
      readonly accountId: number
      readonly login: string
      /** Said when a failed sign-out is not on the gateway's record. */
      readonly unrecorded?: string
      readonly installUrl: string
    }

/** One answer, or the sentence for why there was none. */
export type GithubOutcome =
  | { readonly ok: true; readonly view: GithubView }
  | { readonly ok: false; readonly sentence: string }
