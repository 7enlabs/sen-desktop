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

// Retention bound for the raw log (V-03, Sen issue #65 follow-up).
// Why 10,000: one session_event is appended per validated model event, and
// a turn yields only a handful of events, so even a 1,000-turn session is
// a few thousand events; 10,000 sits an order of magnitude above any
// ordinary session, so no useful history of a normal session is lost.
// Why a bound at all: payloads carry full model text, so an unbounded log
// grows with session length without limit (a days-long session could reach
// gigabytes in memory); at ~2 KB per event, 10,000 events bound memory to
// roughly tens of MB. Eviction is FIFO: the oldest event goes first, the
// surviving events keep their append order, and seq never reuses a number,
// so a gap in seq is the observable trace that eviction happened.
export const MAX_RAW_LOG_EVENTS = 10000;

export class RawLog {
  private events: RawEvent[] = [];
  private nextSeq = 1;
  private readonly maxEvents: number;

  constructor(maxEvents: number = MAX_RAW_LOG_EVENTS) {
    if (!Number.isInteger(maxEvents) || maxEvents < 1) {
      throw new RangeError("maxEvents must be a positive integer");
    }
    this.maxEvents = maxEvents;
  }

  append(sessionId: string, kind: string, payload: unknown): RawEvent {
    const event: RawEvent = { payload, seq: this.nextSeq, sessionId, kind };
    this.nextSeq += 1;
    this.events.push(event);
    if (this.events.length > this.maxEvents) {
      this.events.shift();
    }
    return event;
  }

  eventsFor(sessionId: string): RawEvent[] {
    return this.events.filter((e) => e.sessionId === sessionId);
  }

  // NOT HERE YET: post-delete bench closure AfterDeleteBench on delete. No
  // delete function may be added before that design exists.
  // Having no delete function here is intentional, to avoid unclosed deletes.
  // The cap above is not a delete function and does not change that rule:
  // no caller can delete anything, there is still no public delete API,
  // and eviction happens only inside append, only to the oldest event,
  // only when the fixed bound is exceeded. The post-delete bench design is
  // still required before any delete function is added.
}
