/**
 * The floating panel chrome. Host subscriptions live in adapters; the
 * chrome renders in `ui/`. Product rules live in `conversation/`.
 */
export { useColorScheme } from "./adapters/color-scheme"
export { useEdgeReveal } from "./adapters/edge-reveal"
export { useHostPanel } from "./adapters/host-panel"
export { useSurface } from "./adapters/surface"
export type { Surface } from "./model"
export { App } from "./ui/app"
export { WaveformIcon } from "./ui/waveform-icon"
