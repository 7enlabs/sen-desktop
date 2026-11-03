// STUB - Sen UI shell NOT FILLED IN YET.
// The desktop shell technology is decided: Tauri 2, per Telegram message 4070
// (ZC Leader, 03/11/2026). This file only reserves the place for the app;
// it holds no fake content, never calls the SDK directly, only calls via
// ipc/bridge.ts.
import { invoke } from "../ipc/bridge.ts";

export async function startApp(): Promise<void> {
  // NOT HERE YET: real UI init. No invoke call here until a real session exists.
  void invoke;
}
