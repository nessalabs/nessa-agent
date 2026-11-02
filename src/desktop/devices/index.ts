/**
 * Devices: the hub's machines, as the side rail's Devices page shows them.
 * Composed in `main.tsx` (its gateway) and `desktop-window.tsx` (its page).
 */
export {
  devicesGateway,
  type DevicesClient,
  type DevicesGateway,
} from "./adapters/devices-gateway"
export { DevicesProvider, DevicesView, useDevicesGateway } from "./ui/devices-view"
export { PoolOrder } from "./ui/pool-order"
