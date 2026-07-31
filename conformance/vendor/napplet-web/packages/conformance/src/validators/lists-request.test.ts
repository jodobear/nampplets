import { describe, expect, it } from 'vitest';
import { createReferenceShell } from '../shell/reference-shell.js';
import { validateEnvelope } from './envelope.js';

const releasedRequest = {
  type: 'lists.add',
  id: 'lists-1',
  list: { kind: 10_000 },
  items: [{ itemType: 'pubkey', value: 'a'.repeat(64) }],
};

describe('NAP-LISTS nested request validation', () => {
  it('accepts the released semantic item wire', () => {
    expect(validateEnvelope(releasedRequest).ok).toBe(true);
  });

  it('rejects raw NIP-51 item tags before the reference responder', () => {
    const raw = {
      ...releasedRequest,
      items: [{ type: 'p', value: 'a'.repeat(64) }],
    };
    expect(validateEnvelope(raw).errors).toContainEqual(expect.objectContaining({
      code: 'invalid-lists-request',
      field: 'items[0].itemType',
    }));
    expect(createReferenceShell().handle(raw)).toEqual([]);
  });

  it('rejects malformed selectors, items, and options', () => {
    const invalid = [
      { ...releasedRequest, list: { kind: 10_000, type: 'mute-list' } },
      { ...releasedRequest, list: { kind: '10000' } },
      { ...releasedRequest, items: [] },
      { ...releasedRequest, items: [{ itemType: 'p', value: 'a'.repeat(64) }] },
      { ...releasedRequest, items: [{ itemType: 'pubkey', value: 7 }] },
      { ...releasedRequest, options: { create: 'yes' } },
    ];
    for (const request of invalid) {
      expect(validateEnvelope(request).errors).toContainEqual(expect.objectContaining({
        code: 'invalid-lists-request',
      }));
    }
  });
});
