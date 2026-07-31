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

  it('accepts an agreeing legacy protocol alias without changing explicit routing identity', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      action: 'open',
      protocol: 'napplet:article/read',
    })).ok).toBe(true);
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      convention: 'napplet:article/read',
      protocol: 'napplet:article/read',
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
      [{ archetype: 'note', protocol: false }, 'request.protocol'],
      [{ archetype: 'note', handler: false }, 'request.handler'],
      [{ archetype: 'note', sender: 'forged-source' }, 'request.sender'],
    ] as const) {
      expect(validateEnvelope(intentInvoke(request)).errors).toContainEqual(
        expect.objectContaining({ field }),
      );
    }
  });

  it('rejects unknown top-level fields while leaving payload opaque', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      extra: true,
      payload: { extra: true },
    })).errors).toContainEqual(expect.objectContaining({
      code: 'invalid-intent-request',
      field: 'request.extra',
    }));
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      payload: { extra: true },
      behavior: { focus: true, newWindow: false, reuse: true },
    })).ok).toBe(true);
  });

  it('matches the runtime intent behavior schema exactly', () => {
    for (const [behavior, field] of [
      [null, 'request.behavior'],
      ['focus', 'request.behavior'],
      [{ focus: 'yes' }, 'request.behavior.focus'],
      [{ custom: true }, 'request.behavior.custom'],
    ] as const) {
      expect(validateEnvelope(intentInvoke({ archetype: 'note', behavior })).errors)
        .toContainEqual(expect.objectContaining({ field }));
    }
  });

  it('matches the runtime handler text boundary', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      handler: 'note-viewer',
    })).ok).toBe(true);
    for (const handler of ['', '\0', 'note\nviewer', '\x7f', 'ö'.repeat(513)]) {
      expect(validateEnvelope(intentInvoke({ archetype: 'note', handler })).errors)
        .toContainEqual(expect.objectContaining({
          code: 'invalid-intent-request',
          field: 'request.handler',
        }));
    }
  });

  it('rejects archetypes outside the runtime lowercase slug boundary', () => {
    for (const archetype of ['', 'Note', 'note\nopen', 'nöt', 'a'.repeat(257)]) {
      expect(validateEnvelope(intentInvoke({ archetype })).errors).toContainEqual(
        expect.objectContaining({
          code: 'invalid-intent-request',
          field: 'request.archetype',
        }),
      );
    }
  });

  it('rejects actions outside the runtime lowercase slug boundary', () => {
    for (const action of ['', 'Open', 'note\nopen', 'öppna', 'a'.repeat(257)]) {
      expect(validateEnvelope(intentInvoke({ archetype: 'note', action })).errors).toContainEqual(
        expect.objectContaining({
          code: 'invalid-intent-request',
          field: 'request.action',
        }),
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
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      protocol: 'napplet:note/open?draft=true',
    })).errors).toContainEqual(expect.objectContaining({
      code: 'invalid-intent-request',
      field: 'request.protocol',
    }));
  });

  it('rejects conflicting convention and legacy protocol aliases', () => {
    expect(validateEnvelope(intentInvoke({
      archetype: 'note',
      convention: 'napplet:note/open',
      protocol: 'napplet:article/read',
    }))).toMatchObject({
      ok: false,
      errors: [expect.objectContaining({
        code: 'invalid-intent-request',
        field: 'request.protocol',
      })],
    });
  });
});
