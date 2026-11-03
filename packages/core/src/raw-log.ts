// Log su kien tho: nguon su that, tach khoi lop suy ra, theo prep B-2 muc 1 va diem 855.
// Tom tat compaction chi la khung nhin dan xuat, phai dung lai duoc tu log tho.
// Suy ra mac dinh HUY tru khi qua cong kiem chung. Xoa phai dong kin hau due (AfterDeleteBench).
export interface RawEvent {
  seq: number;
  sessionId: string;
  kind: string;
  payload: unknown;
}

export class RawLog {
  private events: RawEvent[] = [];
  private nextSeq = 1;

  append(sessionId: string, kind: string, payload: unknown): RawEvent {
    const event: RawEvent = { payload, seq: this.nextSeq, sessionId, kind };
    this.nextSeq += 1;
    this.events.push(event);
    return event;
  }

  eventsFor(sessionId: string): RawEvent[] {
    return this.events.filter((e) => e.sessionId === sessionId);
  }

  // CHUA CO: dong kin hau due AfterDeleteBench khi xoa. Ham xoa khong duoc them truoc khi co thiet ke do.
  // Khong co ham xoa o day la co y, de tranh xoa khong dong kin.
}
