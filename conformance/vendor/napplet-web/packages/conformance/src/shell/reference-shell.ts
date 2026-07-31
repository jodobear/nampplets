/**
 * @napplet/conformance -- Reference mock shell.
 *
 * Records and answers napplet envelopes through a transport-agnostic finite
 * host. Focused responder, intent, and delivery modules own their respective
 * protocol behavior.
 *
 * @packageDocumentation
 */

import { validateEnvelope, type EnvelopeVerdict } from '../validators/envelope.js';
import {
  createReferenceDeliveryQueue,
  REFERENCE_DELIVERY_LIMITS,
  type ReferenceDeliveryState,
} from './reference-delivery.js';
import { handleIncEmit, handleIntentInvoke, intentAvailability } from './reference-intent.js';
import { REFERENCE_PUBKEY, RESPONDERS } from './reference-responders.js';

export { REFERENCE_DELIVERY_LIMITS, REFERENCE_PUBKEY };
export type { ReferenceDeliveryState };

/** A source identity supplied by the reference runtime's authenticated endpoint fixture. */
export interface ReferenceEndpoint {
  /** The authenticated source napplet dTag. */
  dTag: string;
}

/** Default authenticated source used by the backwards-compatible handle helper. */
export const REFERENCE_ENDPOINT: ReferenceEndpoint = { dTag: 'reference-source' };

/** One recorded inbound envelope from the napplet, with its validation verdict. */
export interface RecordedEnvelope {
  /** The raw envelope the napplet posted. */
  envelope: unknown;
  /** Verdict from {@link validateEnvelope}. */
  verdict: EnvelopeVerdict;
  /** Monotonic-ish timestamp (ms) when the shell received it. */
  timestamp: number;
}

/** Options for {@link createReferenceShell}. */
export interface ReferenceShellOptions {
  /** Injectable clock for deterministic tests. Defaults to `Date.now`. */
  now?: () => number;
}

/** A reference shell instance. */
export interface ReferenceShell {
  /** All envelopes recorded so far, in arrival order. */
  readonly records: readonly RecordedEnvelope[];
  /** Process one inbound envelope from the default authenticated endpoint. */
  handle(envelope: unknown): unknown[];
  /** Process one inbound envelope from an explicitly authenticated endpoint. */
  handleFrom(endpoint: ReferenceEndpoint, envelope: unknown): unknown[];
  /** Current bounded delivery occupancy and the latest observable refusal. */
  readonly deliveryState: ReferenceDeliveryState;
  /** Drain retained target deliveries for one resolved reference target. */
  takeDeliveries(target: string): unknown[];
  /** Clear recorded envelopes and retained delivery state. */
  reset(): void;
}

const ok = <T extends Record<string, unknown>>(value: T): T[] => [value];

/** Create one isolated, finite reference shell. */
export function createReferenceShell(options: ReferenceShellOptions = {}): ReferenceShell {
  const now = options.now ?? (() => Date.now());
  const records: RecordedEnvelope[] = [];
  const deliveries = createReferenceDeliveryQueue();

  function handleFrom(endpoint: ReferenceEndpoint, envelope: unknown): unknown[] {
    const type =
      envelope && typeof envelope === 'object' && typeof (envelope as Record<string, unknown>).type === 'string'
        ? ((envelope as Record<string, unknown>).type as string)
        : undefined;

    const verdict = validateEnvelope(envelope);
    records.push({ envelope, verdict, timestamp: now() });

    if (!type || !verdict.ok) return [];
    const env = envelope as Record<string, unknown>;
    if (type === 'intent.invoke') return handleIntentInvoke(endpoint, env, deliveries);
    if (type === 'intent.available') {
      return ok({ type: 'intent.available.result', id: env.id, availability: intentAvailability(env.archetype) });
    }
    if (type === 'intent.handlers') {
      return ok({ type: 'intent.handlers.result', id: env.id, handlers: [intentAvailability('note')] });
    }
    if (type === 'inc.emit') {
      handleIncEmit(endpoint, env, deliveries);
      return [];
    }
    const responder = RESPONDERS[type];
    return responder ? responder(env) : [];
  }

  return {
    get records() {
      return records;
    },
    get deliveryState() {
      return deliveries.state;
    },
    handle(envelope) {
      return handleFrom(REFERENCE_ENDPOINT, envelope);
    },
    handleFrom,
    takeDeliveries(target) {
      return deliveries.take(target);
    },
    reset() {
      records.length = 0;
      deliveries.reset();
    },
  };
}

/** Minimal window surface {@link attachReferenceShell} needs (eases testing). */
export interface MessageWindowLike {
  addEventListener(type: 'message', listener: (event: MessageEvent) => void): void;
  removeEventListener(type: 'message', listener: (event: MessageEvent) => void): void;
}

/** Minimal target surface the shell posts responses to. */
export interface PostTargetLike {
  postMessage(message: unknown, targetOrigin: string): void;
}

/** Options for {@link attachReferenceShell}. */
export interface AttachOptions {
  /** The window that receives `message` events (usually the host `window`). */
  host: MessageWindowLike;
  /** The napplet target to post responses to (usually `iframe.contentWindow`). */
  target: PostTargetLike;
  /** Optional source guard for events from the target iframe. */
  expectedSource?: unknown;
  /** Authenticated endpoint identity for messages that pass the web source guard. */
  endpoint?: ReferenceEndpoint;
}

/** Bind a reference shell to a real postMessage channel. */
export function attachReferenceShell(shell: ReferenceShell, options: AttachOptions): () => void {
  const listener = (event: MessageEvent): void => {
    if (options.expectedSource !== undefined && event.source !== options.expectedSource) return;
    for (const response of shell.handleFrom(options.endpoint ?? REFERENCE_ENDPOINT, event.data)) {
      options.target.postMessage(response, '*');
    }
  };
  options.host.addEventListener('message', listener);
  return () => options.host.removeEventListener('message', listener);
}
