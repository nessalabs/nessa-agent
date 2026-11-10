/**
 * Devices: the hub's machines, as the side rail's Devices page shows them.
 * `ui/add-gateway.tsx` creates PeerRead invitations;
 * `adapters/gateway-pairing.ts` owns the separate explicit Human session.
 * `model/gateway-pairing.ts` keeps selected invitation facts together and formats
 * the quoted enrollment command.
 * Phone and worker consent stay with their existing adapters.
 * Composed in `main.tsx` (its gateway) and `desktop-window.tsx` (its page).
 */
export {
  devicesGateway,
  type DevicesClient,
  type DevicesGateway,
} from "./adapters/devices-gateway"
export { DevicesProvider, DevicesView, useDevicesGateway } from "./ui/devices-view"
export { PoolOrder } from "./ui/pool-order"
