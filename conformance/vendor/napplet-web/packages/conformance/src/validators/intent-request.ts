import type { EnvelopeError } from './envelope.js';

const STABLE_CONVENTION = /^napplet:[^/?#\s]+\/[^/?#\s]+$/;
const INTENT_SLUG = /^[a-z0-9._-]{1,256}$/;
const MAXIMUM_INTENT_TEXT_BYTES = 1_024;
const ASCII_CONTROL = /[\x00-\x1f\x7f]/;
const INTENT_REQUEST_FIELDS = new Set([
  'archetype',
  'action',
  'convention',
  'protocol',
  'payload',
  'handler',
  'behavior',
]);
const INTENT_BEHAVIOR_FIELDS = new Set(['focus', 'newWindow', 'reuse']);

function validRuntimeText(value: string): boolean {
  return value.length > 0
    && !ASCII_CONTROL.test(value)
    && new TextEncoder().encode(value).byteLength <= MAXIMUM_INTENT_TEXT_BYTES;
}

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

function validateBehavior(value: unknown, errors: EnvelopeError[]): void {
  if (value === undefined) return;
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    errors.push({
      code: 'wrong-type',
      message: 'Intent request field "behavior" must be an object',
      field: 'request.behavior',
    });
    return;
  }
  for (const [field, hint] of Object.entries(value)) {
    if (!INTENT_BEHAVIOR_FIELDS.has(field)) {
      errors.push({
        code: 'invalid-intent-request',
        message: `Unknown intent behavior field "${field}"`,
        field: `request.behavior.${field}`,
      });
    } else if (typeof hint !== 'boolean') {
      errors.push({
        code: 'wrong-type',
        message: `Intent behavior field "${field}" must be a boolean`,
        field: `request.behavior.${field}`,
      });
    }
  }
}

/** Validate one outbound intent request without coupling routing to payload convention. */
export function validateIntentInvokeRequest(
  request: unknown,
  errors: EnvelopeError[],
): void {
  if (typeof request !== 'object' || request === null || Array.isArray(request)) return;

  const normalized = request as Record<string, unknown>;
  for (const field of Object.keys(normalized)) {
    if (field !== 'sender' && !INTENT_REQUEST_FIELDS.has(field)) {
      errors.push({
        code: 'invalid-intent-request',
        message: `Unknown intent request field "${field}"`,
        field: `request.${field}`,
      });
    }
  }
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
  } else if (!INTENT_SLUG.test(normalized.archetype)) {
    errors.push({
      code: 'invalid-intent-request',
      message: 'Intent request field "archetype" must be a lowercase role slug',
      field: 'request.archetype',
    });
  }

  for (const field of ['action', 'convention', 'protocol', 'handler'] as const) {
    optionalString(normalized, field, errors);
  }
  validateBehavior(normalized.behavior, errors);

  if (typeof normalized.handler === 'string' && !validRuntimeText(normalized.handler)) {
    errors.push({
      code: 'invalid-intent-request',
      message: 'Intent request field "handler" must be nonempty bounded control-free text',
      field: 'request.handler',
    });
  }

  if (typeof normalized.action === 'string' && !INTENT_SLUG.test(normalized.action)) {
    errors.push({
      code: 'invalid-intent-request',
      message: 'Intent request field "action" must be a lowercase role slug',
      field: 'request.action',
    });
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
