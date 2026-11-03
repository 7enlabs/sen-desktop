// TokenEconomizer, theo diem 856 qua prep B-2 muc 4.
// Tieu chi turn ket: cung loi lap lai qua cac lan goi tool, hoac turn dai qua 2,5 lan muc
// thuong trong khi churn cung mot edit, thi redirect toi da 1 lan, sau do chi bao dung.
// Bang chung (chu ky loi + do dai turn) ghi vao log tho.
// STUB: ham phat hien that CHUA LAP DAY, hang so duoi day chi ghi tieu chi tu prep.
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
