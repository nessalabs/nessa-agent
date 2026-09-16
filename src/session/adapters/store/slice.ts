import { createSlice, type PayloadAction } from "@reduxjs/toolkit"
import type { HealthResult, ProductSessionReady } from "@nessa/client"

import { initialSessionState, type SessionState } from "../../model"

const sessionSlice = createSlice({
  name: "session",
  initialState: initialSessionState(),
  reducers: {
    retrySession(state) {
      state.retryRequest += 1
    },
    sessionConnecting(state) {
      state.phase = "connecting"
      state.detail = "Connecting to the local server…"
      state.hello = null
      state.health = null
    },
    sessionReconnecting(state) {
      state.phase = "reconnecting"
      state.detail = "Reconnecting to the local server…"
      state.hello = null
      state.health = null
    },
    sessionReady(
      state,
      action: PayloadAction<{
        hello: ProductSessionReady
        health: HealthResult
      }>,
    ) {
      state.phase = "ready"
      state.detail = "Connected"
      state.hello = action.payload.hello
      state.health = action.payload.health
    },
    sessionError(state, action: PayloadAction<string>) {
      state.phase = "error"
      state.detail = action.payload
      state.hello = null
      state.health = null
    },
  },
})

export const {
  retrySession,
  sessionConnecting,
  sessionReconnecting,
  sessionReady,
  sessionError,
} = sessionSlice.actions

export const sessionReducer = sessionSlice.reducer

export type { SessionState }
