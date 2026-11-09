/**
 * The pool's order, as Devices and a channel's settings edit it: every
 * paired computer, first choice first, and the ones left out. New threads
 * placed on "Any computer" go to the first one in the order that is online
 * and not busy; a left-out computer is never given one. What is saved is
 * the gateway's (`devices.setPool`); this only turns rows into a preference
 * and back.
 */
import type { DeviceStatus, WorkerCard } from "./devices"

/** Which paired computers new work goes to first, and which it never goes to. */
export interface PoolPreference {
  readonly order: readonly string[]
  readonly excluded: readonly string[]
}

/** The most computers one list names: the gateway refuses more. */
export const MAX_POOL_ENTRIES = 64

/** One computer in the order. */
export interface PoolRow {
  /** `peer:<key>`. */
  readonly environment: string
  readonly name: string
  readonly status: DeviceStatus
  /** Whether new threads may go to it. */
  readonly included: boolean
}

/**
 * The rows `preference` puts `workers` in: those it orders, in its order,
 * then the rest in the list's own order, then those it leaves out. A
 * computer it names that is not listed is not shown, and is kept as it was
 * when the rows are saved again ({@link preferenceOf}).
 */
export function poolRows(
  workers: readonly WorkerCard[],
  preference: PoolPreference,
): PoolRow[] {
  const byName = new Map(workers.map((worker) => [worker.environment, worker]))
  const names = new Map<string, number>()
  for (const worker of workers) names.set(worker.name, (names.get(worker.name) ?? 0) + 1)
  // Two computers with one hostname are told apart by their key's start.
  const nameOf = (worker: WorkerCard) =>
    (names.get(worker.name) ?? 0) > 1
      ? `${worker.name} · ${worker.environment.slice("peer:".length, "peer:".length + 6)}`
      : worker.name
  const row = (worker: WorkerCard, included: boolean): PoolRow => ({
    environment: worker.environment,
    name: nameOf(worker),
    status: worker.status,
    included,
  })
  const named = new Set([...preference.order, ...preference.excluded])
  const ordered = preference.order.flatMap((name) => {
    const worker = byName.get(name)
    return worker ? [row(worker, true)] : []
  })
  const rest = workers
    .filter((worker) => !named.has(worker.environment))
    .map((worker) => row(worker, true))
  const left = preference.excluded.flatMap((name) => {
    const worker = byName.get(name)
    return worker ? [row(worker, false)] : []
  })
  return [...ordered, ...rest, ...left]
}

/** `rows` with the one at `index` moved by `by` places, kept in bounds. */
export function moveRow(rows: readonly PoolRow[], index: number, by: number): PoolRow[] {
  const to = Math.max(0, Math.min(rows.length - 1, index + by))
  const moved = [...rows]
  const [row] = moved.splice(index, 1)
  if (row) moved.splice(to, 0, row)
  return moved
}

/** `rows` with the one at `index` let in or left out. */
export function includeRow(
  rows: readonly PoolRow[],
  index: number,
  included: boolean,
): PoolRow[] {
  return rows.map((row, at) => (at === index ? { ...row, included } : row))
}

/**
 * What `rows` save: every computer let in, in the order shown, then those
 * `previous` ordered that are not shown; every one left out, then those
 * `previous` left out that are not shown. Each list keeps at most
 * {@link MAX_POOL_ENTRIES}: computers past it follow the order unnamed.
 */
export function preferenceOf(
  rows: readonly PoolRow[],
  previous: PoolPreference,
): PoolPreference {
  const shown = new Set(rows.map((row) => row.environment))
  const order = [
    ...rows.filter((row) => row.included).map((row) => row.environment),
    ...previous.order.filter((name) => !shown.has(name)),
  ]
  const excluded = [
    ...rows.filter((row) => !row.included).map((row) => row.environment),
    ...previous.excluded.filter((name) => !shown.has(name) && !order.includes(name)),
  ]
  return {
    order: order.slice(0, MAX_POOL_ENTRIES),
    excluded: excluded.slice(0, MAX_POOL_ENTRIES),
  }
}

/** Whether two preferences name the same computers the same way. */
export function samePreference(a: PoolPreference, b: PoolPreference): boolean {
  const same = (x: readonly string[], y: readonly string[]) =>
    x.length === y.length && x.every((name, index) => name === y[index])
  return same(a.order, b.order) && same(a.excluded, b.excluded)
}
