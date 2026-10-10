/** The production composer tray's "Run on" over the hosts a gateway names. */
import * as React from "react"
import { createRoot } from "react-dom/client"
import { ComposerTray } from "../../src/panel/ui/composer-tray"
import "../../src/styles.css"

/** What `agents.list` names under `environments`, by `?case=`. */
const hostCases: Record<string, readonly string[]> = {
  none: [],
  hosts: ["devbox", "me@build-01.internal.example.com"],
}

function Fixture() {
  const name = new URLSearchParams(location.search).get("case") ?? "hosts"
  const hosts = hostCases[name] ?? []
  const [host, setHost] = React.useState<string>()
  return (
    // The tray rises above its +, so the + sits at the foot of the page.
    <main
      data-run-on-fixture
      data-case={name}
      data-chosen={host ?? ""}
      className="flex h-screen flex-col justify-end bg-background p-4"
    >
      <ComposerTray
        disabled={false}
        onChoose={() => {}}
        environment={hosts.length ? { host, hosts, onChange: setHost } : undefined}
      />
    </main>
  )
}

createRoot(document.getElementById("root")!).render(<Fixture />)
