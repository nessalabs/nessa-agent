#!/usr/bin/env node
/** Publish a projection of run-all's existing result through ordinary CI notices. */
import { openSync, readSync, closeSync } from "node:fs"
import {
  desktopReceipt,
  incompleteReceipt,
  receiptNotices,
  receiptMaxRawBytes,
} from "./lib/ci-receipt.mjs"

function readResult(file) {
  const descriptor = openSync(file, "r")
  try {
    const bytes = Buffer.allocUnsafe(receiptMaxRawBytes + 1)
    let used = 0
    while (used < bytes.length) {
      const read = readSync(descriptor, bytes, used, bytes.length - used, null)
      if (read === 0) break
      used += read
    }
    return bytes.subarray(0, used)
  } finally {
    closeSync(descriptor)
  }
}

const [file, section] = process.argv.slice(2)
const source = {
  commit: process.env.GITHUB_SHA,
  run: process.env.GITHUB_RUN_ID,
  attempt: process.env.GITHUB_RUN_ATTEMPT,
}
try {
  if (!file)
    throw new Error("expected result file and manifest or group1, group2, group3")
  let receipt
  try {
    receipt = desktopReceipt(readResult(file), source)
  } catch (error) {
    receipt = incompleteReceipt(source, "unreadable-result")
    console.error(error.message)
  }
  for (const notice of receiptNotices(receipt, section))
    process.stdout.write(notice + "\n")
  if (receipt.manifest.status !== "prepared") {
    console.error("Desktop measurement receipt incomplete: " + receipt.manifest.reason)
    process.exitCode = 1
  }
} catch (error) {
  console.error(error.message)
  process.exitCode = 1
}
