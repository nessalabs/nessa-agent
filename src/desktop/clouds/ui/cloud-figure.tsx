/**
 * A cloud's picture: Boat's boat on its water, Agent37's mark. Drawn in the
 * text colour, so the mark follows the light or dark theme.
 */
import { DeviceFigure } from "@nessa-ui/react/device-figure"
import type { Cloud } from "../model/clouds"
import "./clouds.css"

export function CloudFigure({
  cloud,
  dimmed,
  className,
}: {
  cloud: Cloud
  dimmed?: boolean
  className?: string
}) {
  switch (cloud) {
    case "boat":
      return <DeviceFigure kind="boat" afloat dimmed={dimmed} className={className} />
    case "agent37":
      return (
        <span
          className={["cloud-mark", "cloud-mark-agent37", className]
            .filter(Boolean)
            .join(" ")}
          data-dimmed={dimmed ? "" : undefined}
          aria-hidden="true"
        />
      )
  }
}
