import type { ReferenceDeliveryQueue } from './reference-delivery.js';

const REFERENCE_HANDLER = 'reference-handler';
const REFERENCE_SUBSCRIBER = 'reference-subscriber';
const REFERENCE_CONVENTION = 'napplet:note/open';
const REFERENCE_CONTRACT = { convention: REFERENCE_CONVENTION, eventKinds: [1, 30023] };

export interface ReferenceIntentEndpoint {
  readonly dTag: string;
}

const ok = <T extends Record<string, unknown>>(value: T): T[] => [value];

function unavailableIntent(
  id: unknown,
  archetype: unknown,
  action: unknown,
  error: string,
): unknown[] {
  return ok({
    type: 'intent.invoke.result',
    id,
    result: {
      ok: false,
      handled: false,
      archetype: typeof archetype === 'string' ? archetype : 'invalid',
      action: typeof action === 'string' ? action : 'invalid',
      error,
    },
  });
}

/** Resolve, queue, and answer one normalized intent invocation. */
export function handleIntentInvoke(
  endpoint: ReferenceIntentEndpoint,
  env: Record<string, unknown>,
  deliveries: ReferenceDeliveryQueue,
): unknown[] {
  const request = env.request;
  if (typeof request !== 'object' || request === null || Array.isArray(request)) {
    return unavailableIntent(env.id, undefined, undefined, 'invalid intent request');
  }

  const normalized = request as Record<string, unknown>;
  const { archetype, action, convention } = normalized;
  if (typeof archetype !== 'string' || typeof action !== 'string' || typeof convention !== 'string') {
    return unavailableIntent(
      env.id,
      archetype,
      action,
      'intent request must carry normalized identity',
    );
  }

  const parsed = /^napplet:([^/?#\s]+)\/([^/?#\s]+)$/.exec(convention);
  if (!parsed || parsed[1] !== archetype || parsed[2] !== action) {
    return unavailableIntent(env.id, archetype, action, 'intent request conflicts with its convention');
  }
  if (convention !== REFERENCE_CONVENTION) {
    return unavailableIntent(env.id, archetype, action, 'no reference handler for convention');
  }

  const delivery: Record<string, unknown> = {
    sender: endpoint.dTag,
    archetype,
    action,
    convention,
  };
  if ('payload' in normalized) delivery.payload = normalized.payload;
  if (!deliveries.queue(REFERENCE_HANDLER, { type: 'intent.deliver', delivery })) {
    return unavailableIntent(env.id, archetype, action, 'reference delivery queue saturated');
  }

  return ok({
    type: 'intent.invoke.result',
    id: env.id,
    result: { ok: true, handled: true, archetype, action, convention, handler: REFERENCE_HANDLER },
  });
}

/** Report the deterministic handler catalog for one archetype. */
export function intentAvailability(archetype: unknown): Record<string, unknown> {
  if (archetype !== 'note') {
    return { archetype, available: false, candidates: [], hasDefault: false };
  }
  return {
    archetype,
    available: true,
    candidates: [{
      dTag: REFERENCE_HANDLER,
      actions: ['open'],
      conventions: [REFERENCE_CONVENTION],
      contracts: [REFERENCE_CONTRACT],
      isDefault: true,
    }],
    hasDefault: true,
  };
}

/** Route a reference INC convention event into the same finite delivery queue. */
export function handleIncEmit(
  endpoint: ReferenceIntentEndpoint,
  env: Record<string, unknown>,
  deliveries: ReferenceDeliveryQueue,
): void {
  if (env.topic !== REFERENCE_CONVENTION) return;
  const event: Record<string, unknown> = {
    type: 'inc.event',
    topic: env.topic,
    sender: endpoint.dTag,
  };
  if ('payload' in env) event.payload = env.payload;
  deliveries.queue(REFERENCE_SUBSCRIBER, event);
}
