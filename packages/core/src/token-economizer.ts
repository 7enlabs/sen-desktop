// TokenEconomizer, per point 856 via prep B-2 section 4.
// Stop-turn rule: the same error repeated across tool calls, or a turn longer
// than 2.5 times the typical length while churning the same edit, redirects
// at most 1 time, then only reports a stop.
// Evidence (error signature + turn length) goes into the raw log.
// STUB: the real detection function is NOT FILLED IN YET; the constants below
// only record the rule from the prep.
export const TOKEN_ECONOMIZER_MAX_REDIRECT = 1;
export const TOKEN_ECONOMIZER_LONG_TURN_RATIO = 2.5;

export interface TurnSignal {
  errorSignature: string;
  turnLength: number;
  typicalTurnLength: number;
  sameEditChurn: boolean;
  repeatedError: boolean;
  redirectCount: number;
}

export function shouldStopTurn(signal: TurnSignal): boolean {
  if (signal.redirectCount >= TOKEN_ECONOMIZER_MAX_REDIRECT) {
    return true;
  }
  if (signal.repeatedError) {
    return true;
  }
  if (
    signal.sameEditChurn &&
    signal.typicalTurnLength > 0 &&
    signal.turnLength > TOKEN_ECONOMIZER_LONG_TURN_RATIO * signal.typicalTurnLength
  ) {
    return true;
  }
  return false;
}
