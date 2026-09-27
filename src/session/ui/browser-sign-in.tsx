import type { FormEvent } from "react"

export function BrowserSignIn({
  busy = false,
  error,
  onConnect,
  onRetry,
}: {
  busy?: boolean
  error?: string
  onConnect: (token: string) => void
  onRetry?: () => void
}) {
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const form = event.currentTarget
    const data = new FormData(form)
    onConnect(String(data.get("token") ?? ""))
    form.reset()
  }
  return (
    <main className="nessa-browser-login">
      <form onSubmit={submit} className="nessa-browser-card">
        <h1>Connect to Nessa</h1>
        <p>Use an access token from your local Nessa server.</p>
        <label htmlFor="browser-token">Access token</label>
        <input
          id="browser-token"
          name="token"
          type="password"
          autoComplete="off"
          required
          maxLength={16384}
          disabled={busy}
          spellCheck={false}
        />
        <p>
          Sessions expire after 30 days of inactivity, or sooner if the access token
          expires.
        </p>
        {error && <p role="alert">{error}</p>}
        <button type="submit" disabled={busy}>
          {busy ? "Connecting…" : "Connect"}
        </button>
        {onRetry && (
          <button type="button" onClick={onRetry} disabled={busy}>
            Retry connection
          </button>
        )}
      </form>
    </main>
  )
}
