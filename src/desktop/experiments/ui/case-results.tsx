import { useState } from "react"
import { VirtualList } from "@nessa-ui/react/virtual-list"
import {
  caseTotal,
  type CaseMove,
  type CaseResults,
  type CaseSlice,
  type MovedCase,
} from "../model/experiment"
import { points } from "./format"
import { count, exact } from "../../ui/format"
import { Selector } from "./selector"
import { tooltip } from "../../ui/tooltip"

/** Up to this many cases, every case is a square; past it, the split is a bar. */
const gridLimit = 200
/** Slices listed before the rest fold away. */
const slicesShown = 5
const movedRow = 34

const caseLabels: Record<CaseMove, string> = {
  fixed: "Fixed",
  broke: "Broke",
  passing: "Still passing",
  failing: "Still failing",
}

const order: readonly CaseMove[] = ["fixed", "broke", "passing", "failing"]

/**
 * The test split against the parent, at any size. The four outcomes as one
 * proportion — a square per case while there are few enough to count — then
 * where the movement was, by slice, then the cases that moved, which stay few
 * enough to read even when the suite has ten thousand.
 */
export function CaseResultsView({ results }: { results: CaseResults }) {
  const total = caseTotal(results)
  return (
    <div className="xp-cases">
      <p className="xp-cases-head" {...tooltip(`${exact(total)} test cases`)}>
        <strong>{count(total)}</strong> test cases
      </p>
      {total <= gridLimit ? (
        <CaseGrid results={results} />
      ) : (
        <CaseBar results={results} />
      )}
      <ul className="xp-case-legend">
        {order.map((move) => (
          <li
            key={move}
            {...tooltip(
              `${exact(results.counts[move])} ${caseLabels[move].toLowerCase()}`,
            )}
          >
            <span className="xp-case" data-move={move} aria-hidden />
            <span className="xp-mono">{count(results.counts[move])}</span>
            {caseLabels[move]}
          </li>
        ))}
      </ul>
      {results.slices.length > 0 ? <Slices slices={results.slices} /> : null}
      {results.moved.length > 0 ? <MovedCases moved={results.moved} /> : null}
    </div>
  )
}

/** A square per case, grouped by outcome, for a split small enough to count by eye. */
function CaseGrid({ results }: { results: CaseResults }) {
  const cells = order.flatMap((move) =>
    Array.from({ length: results.counts[move] }, () => move),
  )
  return (
    <div className="xp-case-grid" role="img" aria-label={summary(results)}>
      {cells.map((move, index) => (
        <span key={index} className="xp-case" data-move={move} />
      ))}
    </div>
  )
}

/**
 * The four outcomes as one bar, for a split of any size. Widths follow the
 * log of each count, not the count: in a large suite the cases that moved
 * are a sliver of it, and on a linear bar thousands still passing flattened
 * them to a hairline. The bar shows what there is and roughly how much; the
 * legend and tooltips say exactly.
 */
function CaseBar({ results }: { results: CaseResults }) {
  const weight = (value: number) => (value > 0 ? Math.log10(value + 1) : 0)
  const total = Math.max(
    order.reduce((sum, move) => sum + weight(results.counts[move]), 0),
    1,
  )
  return (
    <div className="xp-case-bar" role="img" aria-label={summary(results)}>
      {order.map((move) =>
        results.counts[move] > 0 ? (
          <span
            key={move}
            data-move={move}
            style={{ flexGrow: weight(results.counts[move]) / total }}
            {...tooltip(
              `${exact(results.counts[move])} ${caseLabels[move].toLowerCase()}`,
            )}
          />
        ) : null,
      )}
    </div>
  )
}

const summary = (results: CaseResults) =>
  order
    .map((move) => `${exact(results.counts[move])} ${caseLabels[move].toLowerCase()}`)
    .join(", ")

/** Where the movement was: each slice's pass rate before and after, the biggest movers first. */
function Slices({ slices }: { slices: readonly CaseSlice[] }) {
  const [all, setAll] = useState(false)
  const rate = (passed: number, of: number) => (of === 0 ? 0 : (passed / of) * 100)
  const sorted = [...slices].sort(
    (a, b) =>
      Math.abs(rate(b.after, b.total) - rate(b.before, b.total)) -
      Math.abs(rate(a.after, a.total) - rate(a.before, a.total)),
  )
  const shown = all ? sorted : sorted.slice(0, slicesShown)
  return (
    <div className="xp-slices">
      <h5>By slice</h5>
      <ul>
        {shown.map((slice) => {
          const before = rate(slice.before, slice.total)
          const after = rate(slice.after, slice.total)
          const change = after - before
          return (
            <li key={slice.name}>
              <span className="xp-truncate">{slice.name}</span>
              <span className="xp-faint" {...tooltip(`${exact(slice.total)} cases`)}>
                {count(slice.total)}
              </span>
              <span className="xp-slice-rate">
                <span className="xp-faint">{Math.round(before)}%</span> →{" "}
                {Math.round(after)}%
              </span>
              <span
                className="xp-delta"
                data-tone={change > 0.5 ? "up" : change < -0.5 ? "down" : "flat"}
              >
                {points(change)}
              </span>
            </li>
          )
        })}
      </ul>
      {slices.length > slicesShown ? (
        <button type="button" className="xp-link" onClick={() => setAll(!all)}>
          {all ? "Fewer slices" : `All ${count(slices.length)} slices`}
        </button>
      ) : null}
    </div>
  )
}

/**
 * The cases that changed outcome, fixed or broken: the ones worth opening.
 * Windowed, so a run that moved a thousand cases scrolls as easily as one
 * that moved three.
 */
function MovedCases({ moved }: { moved: readonly MovedCase[] }) {
  const fixed = moved.filter((each) => each.move === "fixed")
  const broke = moved.filter((each) => each.move === "broke")
  const [show, setShow] = useState<"fixed" | "broke">(
    fixed.length > 0 ? "fixed" : "broke",
  )
  const rows = show === "fixed" ? fixed : broke
  return (
    <div className="xp-moved">
      <div className="xp-moved-head">
        <h5>Cases that moved</h5>
        <Selector
          value={show}
          onChange={setShow}
          label="Show cases"
          size="sm"
          options={[
            { value: "fixed", label: `Fixed ${count(fixed.length)}` },
            { value: "broke", label: `Broke ${count(broke.length)}` },
          ]}
        />
      </div>
      {rows.length === 0 ? (
        <p className="xp-detail-note">None {show === "fixed" ? "fixed" : "broken"}.</p>
      ) : (
        <VirtualList
          className="xp-moved-list"
          items={rows}
          getKey={(item) => item.id}
          rowHeight={movedRow}
          height={Math.min(rows.length * movedRow, 8 * movedRow)}
          aria-label={show === "fixed" ? "Cases fixed" : "Cases broken"}
        >
          {(item) => (
            <div className="xp-moved-row" data-move={item.move}>
              <svg
                className="xp-verdict-mark"
                width="12"
                height="12"
                viewBox="0 0 12 12"
                aria-hidden
              >
                {item.move === "fixed" ? (
                  <path d="M2.8 6.4l2.2 2.2 4.4-5" />
                ) : (
                  <path d="M3.2 3.2l5.6 5.6M8.8 3.2L3.2 8.8" />
                )}
              </svg>
              <span className="xp-moved-id">{item.id}</span>
              <span className="xp-truncate">{item.title}</span>
              <span className="xp-moved-slice xp-truncate">{item.slice}</span>
            </div>
          )}
        </VirtualList>
      )}
    </div>
  )
}
