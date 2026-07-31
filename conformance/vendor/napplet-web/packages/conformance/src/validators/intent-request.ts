import type { EnvelopeError } from './envelope.js';

const STABLE_CONVENTION = /^napplet:[^/?#\s]+\/[^/?#\s]+$/;

function optionalString(
  request: Record<string, unknown>,
  field: 'action' | 'convention' | 'protocol' | 'handler',
  errors: EnvelopeError[],
): void {
  const value = request[field];
  if (value !== undefined && typeof value !== 'string') {
    errors.push({
      code: 'wrong-type',
      message: `Intent request field "${field}" must be a string`,
      field: `request.${field}`,
    });
  }
}

/** Validate one outbound intent request without coupling routing to payload convention. */
export function validateIntentInvokeRequest(
  request: unknown,
  errors: EnvelopeError[],
): void {
  if (typeof request !== 'object' || request === null || Array.isArray(request)) return;

  const normalized = request as Record<string, unknown>;
  if (normalized.archetype === undefined) {
    errors.push({
      code: 'missing-field',
      message: 'Intent request requires a string "archetype" field',
      field: 'request.archetype',
    });
  } else if (typeof normalized.archetype !== 'string') {
    errors.push({
      code: 'wrong-type',
      message: 'Intent request field "archetype" must be a string',
      field: 'request.archetype',
    });
  }

  for (const field of ['action', 'convention', 'protocol', 'handler'] as const) {
    optionalString(normalized, field, errors);
  }

  if ('sender' in normalized) {
    errors.push({
      code: 'forbidden-field',
      message: 'Intent request sender is runtime-derived and cannot be emitted by a napplet',
      field: 'request.sender',
    });
  }

  for (const field of ['convention', 'protocol'] as const) {
    const convention = normalized[field];
    if (typeof convention === 'string' && !STABLE_CONVENTION.test(convention)) {
      errors.push({
        code: 'invalid-intent-request',
        message: 'Intent convention must be a stable queryless napplet convention',
        field: `request.${field}`,
      });
    }
  }

  if (
    typeof normalized.convention === 'string'
    && typeof normalized.protocol === 'string'
    && normalized.convention !== normalized.protocol
  ) {
    errors.push({
      code: 'invalid-intent-request',
      message: 'Intent convention and its legacy protocol alias must agree',
      field: 'request.protocol',
    });
  }
}
