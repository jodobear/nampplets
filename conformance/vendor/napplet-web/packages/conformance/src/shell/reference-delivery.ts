/** Fixed retained-delivery bounds used by reference-shell and browser boot runs. */
export const REFERENCE_DELIVERY_LIMITS = Object.freeze({ perTarget: 32, total: 48 });

/** Bounded host-observable state for retained target delivery saturation. */
export interface ReferenceDeliveryState {
  readonly retained: number;
  readonly refused: number;
  readonly lastRefusal: {
    readonly target: string;
    readonly reason: 'per-target-limit' | 'global-limit';
  } | null;
}

export interface ReferenceDeliveryQueue {
  readonly state: ReferenceDeliveryState;
  queue(target: string, delivery: unknown): boolean;
  take(target: string): unknown[];
  reset(): void;
}

/** Create one finite retained-delivery queue for a reference shell instance. */
export function createReferenceDeliveryQueue(): ReferenceDeliveryQueue {
  const targetQueues = new Map<string, unknown[]>();
  let retained = 0;
  let refused = 0;
  let lastRefusal: ReferenceDeliveryState['lastRefusal'] = null;

  return {
    get state() {
      return {
        retained,
        refused,
        lastRefusal: lastRefusal ? { ...lastRefusal } : null,
      };
    },
    queue(target, delivery) {
      const queue = targetQueues.get(target) ?? [];
      const reason =
        queue.length >= REFERENCE_DELIVERY_LIMITS.perTarget
          ? 'per-target-limit'
          : retained >= REFERENCE_DELIVERY_LIMITS.total
            ? 'global-limit'
            : null;
      if (reason) {
        refused += 1;
        lastRefusal = { target, reason };
        return false;
      }
      queue.push(delivery);
      targetQueues.set(target, queue);
      retained += 1;
      return true;
    },
    take(target) {
      const queue = targetQueues.get(target) ?? [];
      targetQueues.delete(target);
      retained -= queue.length;
      return queue;
    },
    reset() {
      targetQueues.clear();
      retained = 0;
      refused = 0;
      lastRefusal = null;
    },
  };
}
