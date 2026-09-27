import { useState } from "react"
import { greetingFor } from "../model/greeting"
import { Composer } from "./composer"

/**
 * The first thing the window shows: a greeting, and the composer beneath it.
 */
export function Home() {
  const [greeting] = useState(() => greetingFor(new Date().getHours()))

  return (
    <div className="desktop-home">
      <div className="desktop-home-inner">
        <h1 className="desktop-greeting">{greeting}</h1>
        <Composer />
      </div>
    </div>
  )
}
