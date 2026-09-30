import { useState } from "react"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TableSortButton,
} from "@nessa-ui/react/table"
import { testDelta, trainDelta, type Experiment, type Run } from "../model/experiment"
import { AreaMenu } from "./area-menu"
import { Delta } from "./climb-chart"
import { dollars, score } from "./format"
import { ago } from "../../ui/format"
import { AgentAvatar, AreaMark, Meter, VerdictPill } from "./parts"
import { Selector } from "./selector"
import { tooltip } from "../../ui/tooltip"

type Show = "all" | "kept" | "reverted" | "live"
type Sort = "newest" | "test" | "delta"

const shown: Record<Show, (run: Run) => boolean> = {
  all: () => true,
  kept: (run) => run.verdict === "kept",
  reverted: (run) =>
    run.verdict === "flat" ||
    run.verdict === "regressed" ||
    run.verdict === "overfit" ||
    run.verdict === "costly",
  live: (run) => run.verdict === "running" || run.verdict === "queued",
}

/**
 * Every run, filterable by outcome and area, sortable by what it scored. The
 * view holds still — filters above, column names pinned — and only the rows
 * scroll, inside the design system's table.
 */
export function Runs({
  experiment,
  selected,
  onPick,
}: {
  experiment: Experiment
  selected: string | null
  onPick: (runId: string) => void
}) {
  const [show, setShow] = useState<Show>("all")
  const [area, setArea] = useState<string | null>(null)
  const [sort, setSort] = useState<Sort>("newest")
  const rows = experiment.runs
    .filter(shown[show])
    .filter((run) => area === null || run.areaId === area)
    .sort((a, b) => {
      if (sort === "test") return (b.test?.mean ?? -1) - (a.test?.mean ?? -1)
      if (sort === "delta")
        return (testDelta(experiment, b) ?? -99) - (testDelta(experiment, a) ?? -99)
      return b.number - a.number
    })
  const sortable = (label: string, key: Sort) => (
    <TableHead
      className="bg-(--xp-head-fill) xp-num"
      aria-sort={sort === key ? "descending" : undefined}
    >
      <TableSortButton
        direction={sort === key ? "descending" : undefined}
        onClick={() => setSort(sort === key ? "newest" : key)}
      >
        {label}
      </TableSortButton>
    </TableHead>
  )
  return (
    <div className="xp-runs">
      <div className="xp-toolbar">
        <Selector
          value={show}
          onChange={setShow}
          label="Show runs"
          size="sm"
          options={[
            { value: "all", label: "All" },
            { value: "kept", label: "Kept" },
            { value: "reverted", label: "Reverted" },
            { value: "live", label: "Live" },
          ]}
        />
        <AreaMenu experiment={experiment} value={area} onChange={setArea} />
      </div>
      <div className="xp-card xp-card-flush xp-table-card">
        <Table
          className="xp-table"
          containerClassName="xp-table-scroll"
          containerLabel="Runs"
        >
          <TableHeader sticky>
            <TableRow>
              <TableHead className="bg-(--xp-head-fill) xp-num xp-hide-tiny">#</TableHead>
              <TableHead className="bg-(--xp-head-fill)">Change</TableHead>
              <TableHead className="bg-(--xp-head-fill) xp-hide-narrow">Agent</TableHead>
              {sortable("Test", "test")}
              {sortable("Δ test", "delta")}
              <TableHead className="bg-(--xp-head-fill) xp-num xp-hide-narrow">
                Δ train
              </TableHead>
              <TableHead className="bg-(--xp-head-fill) xp-num xp-hide-narrow">
                Cost
              </TableHead>
              <TableHead className="bg-(--xp-head-fill)">Verdict</TableHead>
              <TableHead className="bg-(--xp-head-fill) xp-num xp-hide-narrow">
                When
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((run) => {
              const runArea = experiment.areas.find((each) => each.id === run.areaId)
              const agent = experiment.agents.find((each) => each.id === run.agentId)
              return (
                <TableRow
                  key={run.id}
                  data-state={selected === run.id ? "selected" : undefined}
                  data-verdict={run.verdict}
                  tabIndex={0}
                  onClick={() => onPick(run.id)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault()
                      onPick(run.id)
                    }
                  }}
                >
                  <TableCell className="xp-num xp-faint xp-hide-tiny">
                    {run.number}
                  </TableCell>
                  <TableCell className="xp-change">
                    {/* Wide: the title over its area. Narrow: the area's mark before the title. */}
                    <span className="xp-change-title">
                      <span className="xp-change-mark" {...tooltip(runArea?.name ?? "")}>
                        <AreaMark area={runArea} size={12} />
                      </span>
                      {run.title}
                    </span>
                    <span className="xp-change-area">
                      <AreaMark area={runArea} size={11} />
                      {runArea?.name}
                    </span>
                  </TableCell>
                  <TableCell className="xp-hide-narrow">
                    <span className="xp-agent-cell">
                      <AgentAvatar seed={run.agentId} size={16} />
                      {agent?.name}
                    </span>
                  </TableCell>
                  <TableCell className="xp-num">
                    {run.test ? (
                      score(run.test.mean)
                    ) : run.progress ? (
                      <Meter
                        value={run.progress.done / run.progress.total}
                        label="Evaluation progress"
                      />
                    ) : (
                      <span className="xp-faint">—</span>
                    )}
                  </TableCell>
                  <TableCell className="xp-num">
                    <Delta value={testDelta(experiment, run)} noise={experiment.noise} />
                  </TableCell>
                  <TableCell className="xp-num xp-hide-narrow">
                    <Delta value={trainDelta(experiment, run)} noise={experiment.noise} />
                  </TableCell>
                  <TableCell className="xp-num xp-hide-narrow xp-muted">
                    {run.cost ? dollars(run.cost) : "—"}
                  </TableCell>
                  <TableCell>
                    <VerdictPill verdict={run.verdict} />
                  </TableCell>
                  <TableCell className="xp-num xp-hide-narrow xp-faint">
                    {ago(run.startedAt, experiment.asOf)}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
        {rows.length === 0 ? <p className="xp-empty">No runs match.</p> : null}
      </div>
    </div>
  )
}
