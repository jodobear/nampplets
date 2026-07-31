import { describe, expect, it } from 'vitest';

import { createReferenceShell } from './reference-shell.js';

const authenticatedSource = { dTag: 'authenticated-source' };

describe('reference intent resolution', () => {
  it('routes a registered alternate convention independently of archetype and action', () => {
    const shell = createReferenceShell();

    expect(shell.handleFrom(authenticatedSource, {
      type: 'intent.invoke',
      id: 'intent-alternate-convention',
      request: {
        archetype: 'note',
        action: 'open',
        convention: 'napplet:article/read',
        handler: 'reference-handler',
        payload: { event: 'abc123' },
      },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'intent-alternate-convention',
      result: {
        ok: true,
        handled: true,
        archetype: 'note',
        action: 'open',
        convention: 'napplet:article/read',
        handler: 'reference-handler',
      },
    }]);
    expect(shell.takeDeliveries('reference-handler')).toEqual([{
      type: 'intent.deliver',
      delivery: {
        sender: 'authenticated-source',
        archetype: 'note',
        action: 'open',
        convention: 'napplet:article/read',
        payload: { event: 'abc123' },
      },
    }]);
  });

  it('routes the legacy protocol alias as the same explicit convention identity', () => {
    const shell = createReferenceShell();

    expect(shell.handleFrom(authenticatedSource, {
      type: 'intent.invoke',
      id: 'intent-protocol-alias',
      request: {
        archetype: 'note',
        action: 'open',
        protocol: 'napplet:article/read',
      },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'intent-protocol-alias',
      result: {
        ok: true,
        handled: true,
        archetype: 'note',
        action: 'open',
        convention: 'napplet:article/read',
        handler: 'reference-handler',
      },
    }]);
    expect(shell.takeDeliveries('reference-handler')).toEqual([{
      type: 'intent.deliver',
      delivery: {
        sender: 'authenticated-source',
        archetype: 'note',
        action: 'open',
        convention: 'napplet:article/read',
      },
    }]);
  });

  it('rejects conflicting convention aliases before queuing any delivery', () => {
    const shell = createReferenceShell();

    expect(shell.handleFrom(authenticatedSource, {
      type: 'intent.invoke',
      id: 'intent-conflicting-aliases',
      request: {
        archetype: 'note',
        action: 'open',
        convention: 'napplet:note/open',
        protocol: 'napplet:article/read',
      },
    })).toEqual([]);
    expect(shell.records.at(-1)?.verdict).toMatchObject({
      ok: false,
      errors: [expect.objectContaining({
        code: 'invalid-intent-request',
        field: 'request.protocol',
      })],
    });
    expect(shell.takeDeliveries('reference-handler')).toEqual([]);
  });

  it('refuses an unavailable explicit handler without accepting or queuing delivery', () => {
    const shell = createReferenceShell();

    expect(shell.handleFrom(authenticatedSource, {
      type: 'intent.invoke',
      id: 'intent-unavailable-handler',
      request: {
        archetype: 'note',
        action: 'open',
        convention: 'napplet:note/open',
        handler: 'missing-handler',
      },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'intent-unavailable-handler',
      result: {
        ok: false,
        handled: false,
        archetype: 'note',
        action: 'open',
        error: 'requested intent handler is unavailable',
      },
    }]);
    expect(shell.takeDeliveries('reference-handler')).toEqual([]);
    expect(shell.takeDeliveries('missing-handler')).toEqual([]);
  });

  it('defaults convention only after resolving the supported archetype and action', () => {
    const shell = createReferenceShell();

    expect(shell.handle({
      type: 'intent.invoke',
      id: 'intent-unsupported-action',
      request: { archetype: 'note', action: 'edit' },
    })).toEqual([{
      type: 'intent.invoke.result',
      id: 'intent-unsupported-action',
      result: {
        ok: false,
        handled: false,
        archetype: 'note',
        action: 'edit',
        error: 'no reference handler for archetype and action',
      },
    }]);
    expect(shell.takeDeliveries('reference-handler')).toEqual([]);
  });
});
