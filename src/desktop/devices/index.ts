/**
 * Devices: the hub's machines, as the side rail's Devices page shows them.
 * Composed in `main.tsx` (its gateway) and `desktop-window.tsx` (its page).
 */
export {
  devicesGateway,
  type DevicesClient,
  type DevicesGateway,
} from "./adapters/devices-gateway"
export { DevicesProvider, DevicesView } from "./ui/devices-view"
