import { describe, expect, it } from 'vitest';

import { validateEnvelope } from './envelope.js';

const intentInvoke = (request: unknown) => ({
  type: 'intent.invoke',
  id: 'intent-regression',
  request,
});

describe('intent.invoke request validation', () => {
  it('accepts omitted optional identity fields and opaque payloads', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      payload: { nested: ['opaque', { values: true }] },
    }))).toMatchObject({
      ok: true,
      type: 'intent.invoke',
      direction: 'out',
      errors: [],
    });
  });

  it('keeps stable conventions independent from archetype and action', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      action: 'open',
      convention: 'napplet:article/read',
    })).ok).toBe(true);
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      action: 'edit',
      convention: 'napplet:profile/view',
    })).ok).toBe(true);
  });

  it('rejects missing, malformed, and caller-forged fields', () => {
    const malformedRequest = validateEnvelope(intentInvoke([]));
    expect(malformedRequest.errors).toContainEqual(expect.objectContaining({
      code: 'wrong-type',
      field: 'request',
    }));

    for (const [request, field] of [
      [{}, 'request.archetype'],
      [{ archetype: 1 }, 'request.archetype'],
      [{ archetype: 'note', action: 1 }, 'request.action'],
      [{ archetype: 'note', convention: false }, 'request.convention'],
      [{ archetype: 'note', handler: false }, 'request.handler'],
      [{ archetype: 'note', sender: 'forged-source' }, 'request.sender'],
    ] as const) {
      expect(validateEnvelope(intentInvoke(request)).errors).toContainEqual(
        expect.objectContaining({ field }),
      );
    }
  });

  it('rejects unstable convention URI carriers without imposing routing identity', () => {
    for (const convention of [
      'napplet:note/open?draft=true',
      'napplet:note/open#preview',
      'https://example.com/note/open',
    ]) {
      expect(validateEnvelope(intentInvoke({ archetype: 'note', convention })).errors)
        .toContainEqual(expect.objectContaining({
          code: 'invalid-intent-request',
          field: 'request.convention',
        }));
    }
  });
});
