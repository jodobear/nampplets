import type { EnvelopeError } from './envelope.js';

const LIST_FIELDS = new Set(['kind', 'type', 'identifier']);
const LIST_ITEM_FIELDS = new Set(['itemType', 'value', 'relay', 'label', 'visibility']);
const LIST_OPTION_FIELDS = new Set(['create', 'title', 'description', 'image']);
const LIST_ITEM_TYPES = new Set([
  'pubkey',
  'event',
  'address',
  'hashtag',
  'word',
  'relay',
  'emoji',
  'server',
  'url',
  'group',
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function invalid(errors: EnvelopeError[], field: string, message: string): void {
  errors.push({ code: 'invalid-lists-request', field, message });
}

function validateListRef(value: unknown, errors: EnvelopeError[]): void {
  if (!isRecord(value)) return;
  for (const field of Object.keys(value)) {
    if (!LIST_FIELDS.has(field)) invalid(errors, `list.${field}`, `Unknown list reference field "${field}"`);
  }
  const hasKind = value.kind !== undefined;
  const hasType = value.type !== undefined;
  if (hasKind === hasType) {
    invalid(errors, 'list', 'List reference must carry exactly one of kind or type');
  }
  if (hasKind && (
    typeof value.kind !== 'number'
    || !Number.isInteger(value.kind)
    || value.kind < 0
    || value.kind > 65_535
  )) {
    invalid(errors, 'list.kind', 'List kind must be an unsigned 16-bit integer');
  }
  if (hasType && (typeof value.type !== 'string' || value.type.length === 0)) {
    invalid(errors, 'list.type', 'List type must be a nonempty string');
  }
  if (value.identifier !== undefined && typeof value.identifier !== 'string') {
    invalid(errors, 'list.identifier', 'List identifier must be a string');
  }
}

function validateItem(value: unknown, index: number, errors: EnvelopeError[]): void {
  const prefix = `items[${index}]`;
  if (!isRecord(value)) {
    invalid(errors, prefix, 'List item must be an object');
    return;
  }
  for (const field of Object.keys(value)) {
    if (!LIST_ITEM_FIELDS.has(field)) invalid(errors, `${prefix}.${field}`, `Unknown list item field "${field}"`);
  }
  if (typeof value.itemType !== 'string' || !LIST_ITEM_TYPES.has(value.itemType)) {
    invalid(errors, `${prefix}.itemType`, 'List itemType must use the released semantic wire name');
  }
  if (typeof value.value !== 'string') {
    invalid(errors, `${prefix}.value`, 'List item value must be a string');
  }
  for (const field of ['relay', 'label'] as const) {
    if (value[field] !== undefined && typeof value[field] !== 'string') {
      invalid(errors, `${prefix}.${field}`, `List item ${field} must be a string`);
    }
  }
  if (
    value.visibility !== undefined
    && value.visibility !== 'public'
    && value.visibility !== 'private'
  ) {
    invalid(errors, `${prefix}.visibility`, 'List item visibility must be public or private');
  }
}

function validateOptions(value: unknown, errors: EnvelopeError[]): void {
  if (value === undefined) return;
  if (!isRecord(value)) {
    invalid(errors, 'options', 'List options must be an object');
    return;
  }
  for (const field of Object.keys(value)) {
    if (!LIST_OPTION_FIELDS.has(field)) invalid(errors, `options.${field}`, `Unknown list option field "${field}"`);
  }
  if (value.create !== undefined && typeof value.create !== 'boolean') {
    invalid(errors, 'options.create', 'List create option must be boolean');
  }
  for (const field of ['title', 'description', 'image'] as const) {
    if (value[field] !== undefined && typeof value[field] !== 'string') {
      invalid(errors, `options.${field}`, `List ${field} option must be a string`);
    }
  }
}

/** Validate released NAP-LISTS nested wire objects before mock-shell dispatch. */
export function validateListsMutationRequest(
  envelope: Record<string, unknown>,
  errors: EnvelopeError[],
): void {
  validateListRef(envelope.list, errors);
  if (Array.isArray(envelope.items)) {
    if (envelope.items.length === 0) invalid(errors, 'items', 'List items must not be empty');
    envelope.items.forEach((item, index) => validateItem(item, index, errors));
  }
  validateOptions(envelope.options, errors);
}
