import type { ReferenceDeliveryQueue } from './reference-delivery.js';

const REFERENCE_HANDLER = 'reference-handler';
const REFERENCE_SUBSCRIBER = 'reference-subscriber';
const REFERENCE_CONVENTION = 'napplet:note/open';
const REFERENCE_ALTERNATE_CONVENTION = 'napplet:article/read';
const REFERENCE_CONVENTIONS = [REFERENCE_CONVENTION, REFERENCE_ALTERNATE_CONVENTION] as const;
const REFERENCE_CONTRACTS = [
  { convention: REFERENCE_CONVENTION, eventKinds: [1, 30023] },
  { convention: REFERENCE_ALTERNATE_CONVENTION, eventKinds: [30023] },
];

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
  const archetype = normalized.archetype;
  const action = normalized.action ?? 'open';
  if (typeof archetype !== 'string' || typeof action !== 'string') {
    return unavailableIntent(
      env.id,
      archetype,
      action,
      'intent request must carry a valid archetype and optional action',
    );
  }

  if (archetype !== 'note' || action !== 'open') {
    return unavailableIntent(env.id, archetype, action, 'no reference handler for archetype and action');
  }

  const requestedHandler = normalized.handler ?? 'default';
  if (typeof requestedHandler !== 'string') {
    return unavailableIntent(env.id, archetype, action, 'intent handler must be a string');
  }
  if (
    requestedHandler !== 'default'
    && requestedHandler !== 'choose'
    && requestedHandler !== REFERENCE_HANDLER
  ) {
    return unavailableIntent(env.id, archetype, action, 'requested intent handler is unavailable');
  }

  const convention = normalized.convention ?? REFERENCE_CONVENTION;
  if (typeof convention !== 'string') {
    return unavailableIntent(env.id, archetype, action, 'intent convention must be a string');
  }
  if (!(REFERENCE_CONVENTIONS as readonly string[]).includes(convention)) {
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
      conventions: [...REFERENCE_CONVENTIONS],
      contracts: REFERENCE_CONTRACTS,
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
