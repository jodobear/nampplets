import { describe, expect, it } from 'vitest';
import { validateEnvelope } from './envelope.js';

const invoke = (request: Record<string, unknown>) => ({
  type: 'intent.invoke',
  id: 'intent-1',
  request,
});

describe('intent.invoke request validation', () => {
  it('accepts the archetype-only contract and defaults the omitted action to open', () => {
    expect(validateEnvelope(invoke({ archetype: 'note' }))).toMatchObject({
      ok: true,
      type: 'intent.invoke',
      direction: 'out',
      errors: [],
    });
  });

  it('accepts optional action and convention independently when valid', () => {
    expect(validateEnvelope(invoke({ archetype: 'note', action: 'edit' })).ok).toBe(true);
    expect(validateEnvelope(invoke({
      archetype: 'note',
      convention: 'napplet:note/open',
    })).ok).toBe(true);
    expect(validateEnvelope(invoke({
      archetype: 'note',
      action: 'edit',
      convention: 'napplet:note/edit',
    })).ok).toBe(true);
  });

  it('rejects malformed optional fields without making them required', () => {
    const wrongAction = validateEnvelope(invoke({ archetype: 'note', action: 1 }));
    expect(wrongAction.errors).toContainEqual(expect.objectContaining({
      code: 'wrong-type',
      field: 'request.action',
    }));

    const wrongConvention = validateEnvelope(invoke({ archetype: 'note', convention: false }));
    expect(wrongConvention.errors).toContainEqual(expect.objectContaining({
      code: 'wrong-type',
      field: 'request.convention',
    }));
  });

  it('binds an explicit convention to the explicit or defaulted action', () => {
    for (const request of [
      { archetype: 'note', convention: 'napplet:note/edit' },
      { archetype: 'note', action: 'edit', convention: 'napplet:note/open' },
      { archetype: 'note', convention: 'napplet:note/open?draft=true' },
    ]) {
      expect(validateEnvelope(invoke(request)).errors).toContainEqual(expect.objectContaining({
        code: 'invalid-intent-request',
        field: 'request.convention',
      }));
    }
  });
});
