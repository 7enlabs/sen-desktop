// Raw event log: source of truth, separate from the derived layer, per prep
// B-2 section 1 and point 855.
// A compaction summary is only a derived view; it must be rebuildable from the
// raw log.
// Derived output defaults to DISCARD unless it passes the verification gate.
// Deletion must close the post-delete bench (AfterDeleteBench).
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

  // NOT HERE YET: post-delete bench closure AfterDeleteBench on delete. No
  // delete function may be added before that design exists.
  // Having no delete function here is intentional, to avoid unclosed deletes.
}
