import { createSlice, type PayloadAction } from "@reduxjs/toolkit"
import type { HealthResult, HelloOk } from "@nessa/client"

import { initialSessionState, type SessionState } from "../../model"

const sessionSlice = createSlice({
  name: "session",
  initialState: initialSessionState(),
  reducers: {
    sessionConnecting(state) {
      state.phase = "connecting"
      state.detail = "Connecting to the local server…"
      state.hello = null
      state.health = null
    },
    sessionReady(state, action: PayloadAction<{ hello: HelloOk; health: HealthResult }>) {
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
    sessionDisconnected(state) {
      state.phase = "error"
      state.detail = "Disconnected from the server. Run just server."
      state.hello = null
      state.health = null
    },
  },
})

export const { sessionConnecting, sessionReady, sessionError, sessionDisconnected } =
  sessionSlice.actions

export const sessionReducer = sessionSlice.reducer

export type { SessionState }
