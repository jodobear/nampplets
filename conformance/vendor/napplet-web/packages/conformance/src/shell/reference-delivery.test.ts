import { describe, expect, it } from 'vitest';

import {
  createReferenceShell,
  REFERENCE_DELIVERY_LIMITS,
} from './reference-shell.js';

const intentInvoke = (id: string) => ({
  type: 'intent.invoke',
  id,
  request: {
    archetype: 'note',
    action: 'open',
    convention: 'napplet:note/open',
  },
});
describe('reference shell retained deliveries', () => {
  it('defaults an archetype-only intent to open and its reference convention', () => {
    const shell = createReferenceShell();

    expect(shell.handle({
      type: 'intent.invoke',
      id: 'legacy-defaults',
      request: { archetype: 'note' },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'legacy-defaults',
      result: {
        ok: true,
        handled: true,
        archetype: 'note',
        action: 'open',
        convention: 'napplet:note/open',
        handler: 'reference-handler',
      },
    }]);
    expect(shell.takeDeliveries('reference-handler')).toEqual([{
      type: 'intent.deliver',
      delivery: {
        sender: 'reference-source',
        archetype: 'note',
        action: 'open',
        convention: 'napplet:note/open',
      },
    }]);
  });

  it('reports handled true on delivery and handled false on refusal', () => {
    const shell = createReferenceShell();

    expect(shell.handle(intentInvoke('success'))).toEqual([{
      type: 'intent.invoke.result',
      id: 'success',
      result: {
        ok: true,
        handled: true,
        archetype: 'note',
        action: 'open',
        convention: 'napplet:note/open',
        handler: 'reference-handler',
      },
    }]);
    expect(shell.handle({
      type: 'intent.invoke',
      id: 'refused',
      request: { archetype: 'profile', action: 'open', convention: 'napplet:profile/open' },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'refused',
      result: {
        ok: false,
        handled: false,
        archetype: 'profile',
        action: 'open',
        error: 'no reference handler for convention',
      },
    }]);
  });

  it('refuses intent delivery at the fixed per-target bound without retaining overflow', () => {
    const shell = createReferenceShell();

    for (let index = 0; index < REFERENCE_DELIVERY_LIMITS.perTarget; index += 1) {
      const [response] = shell.handle(intentInvoke(`intent-${index}`)) as Array<{
        result: { handled: boolean };
      }>;
      expect(response.result.handled).toBe(true);
    }

    expect(shell.handle(intentInvoke('overflow'))).toEqual([{
      type: 'intent.invoke.result',
      id: 'overflow',
      result: {
        ok: false,
        handled: false,
        archetype: 'note',
        action: 'open',
        error: 'reference delivery queue saturated',
      },
    }]);
    expect(shell.deliveryState).toEqual({
      retained: REFERENCE_DELIVERY_LIMITS.perTarget,
      refused: 1,
      lastRefusal: { target: 'reference-handler', reason: 'per-target-limit' },
    });
    expect(shell.takeDeliveries('reference-handler')).toHaveLength(
      REFERENCE_DELIVERY_LIMITS.perTarget,
    );
    expect(shell.deliveryState.retained).toBe(0);
  });

  it('bounds combined invoke and INC retention and exposes fire-and-forget refusal', () => {
    const shell = createReferenceShell();
    const subscriberCapacity =
      REFERENCE_DELIVERY_LIMITS.total - REFERENCE_DELIVERY_LIMITS.perTarget;

    for (let index = 0; index < REFERENCE_DELIVERY_LIMITS.perTarget; index += 1) {
      shell.handle(intentInvoke(`intent-${index}`));
    }
    for (let index = 0; index < subscriberCapacity; index += 1) {
      expect(shell.handleFrom(
        { dTag: `sender-${index}` },
        { type: 'inc.emit', topic: 'napplet:note/open', payload: index },
      )).toEqual([]);
    }

    expect(shell.deliveryState.retained).toBe(REFERENCE_DELIVERY_LIMITS.total);
    expect(shell.handleFrom(
      { dTag: 'overflow-sender' },
      { type: 'inc.emit', topic: 'napplet:note/open', payload: 'overflow' },
    )).toEqual([]);
    expect(shell.deliveryState).toEqual({
      retained: REFERENCE_DELIVERY_LIMITS.total,
      refused: 1,
      lastRefusal: { target: 'reference-subscriber', reason: 'global-limit' },
    });
    expect(shell.takeDeliveries('reference-subscriber')).toHaveLength(subscriberCapacity);
  });

  it('reset clears retained deliveries and saturation evidence', () => {
    const shell = createReferenceShell();
    shell.handle(intentInvoke('queued'));

    shell.reset();

    expect(shell.deliveryState).toEqual({ retained: 0, refused: 0, lastRefusal: null });
    expect(shell.takeDeliveries('reference-handler')).toEqual([]);
  });
});
