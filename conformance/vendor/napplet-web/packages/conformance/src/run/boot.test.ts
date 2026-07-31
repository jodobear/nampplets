// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { REFERENCE_DELIVERY_LIMITS } from '../shell/reference-shell.js';
import { bootAndCollect, runtimePrelude } from './boot.js';

describe('bootAndCollect - DOM wiring', () => {
  it('injects the runtime namespace and cleans up the iframe', async () => {
    const before = document.querySelectorAll('iframe').length;
    const boot = await bootAndCollect({
      url: 'about:blank',
      readyTimeoutMs: 60,
      settleMs: 0,
      runDegraded: false,
    });

    expect(boot).toMatchObject({
      installedGlobal: true,
      bootError: null,
      emitted: [],
      deliveryState: { retained: 0, refused: 0, lastRefusal: null },
      degraded: null,
      degradedDeliveryState: null,
    });
    expect(document.querySelectorAll('iframe').length).toBe(before);
  });

  it('returns observable refusal evidence when fire-and-forget INC delivery saturates', async () => {
    const bootPromise = bootAndCollect({
      url: 'about:blank',
      readyTimeoutMs: 100,
      settleMs: 10,
      runDegraded: false,
    });
    const iframe = document.querySelector('iframe');
    expect(iframe?.contentWindow).toBeTruthy();
    for (let index = 0; index <= REFERENCE_DELIVERY_LIMITS.perTarget; index += 1) {
      window.dispatchEvent(new MessageEvent('message', {
        source: iframe!.contentWindow,
        data: { type: 'inc.emit', topic: 'napplet:note/open', payload: index },
      }));
    }
    const boot = await bootPromise;

    expect(boot.bootError).toBeNull();
    expect(boot.emitted).toHaveLength(REFERENCE_DELIVERY_LIMITS.perTarget + 1);
    expect(boot.deliveryState).toEqual({
      retained: REFERENCE_DELIVERY_LIMITS.perTarget,
      refused: 1,
      lastRefusal: { target: 'reference-subscriber', reason: 'per-target-limit' },
    });
  });

  it('resource prelude methods emit envelopes and resolve shell results', async () => {
    const posted: Array<Record<string, unknown>> = [];
    const previousParent = window.parent;
    Object.defineProperty(window, 'parent', {
      configurable: true,
      value: { postMessage: (message: Record<string, unknown>) => posted.push(message) },
    });

    try {
      Function(runtimePrelude(['resource']))();
      const napplet = (window as unknown as {
        napplet: { resource: { bytes(url: string): Promise<Blob> } };
      }).napplet;
      const promise = napplet.resource.bytes('data:text/plain;base64,aGk=');

      expect(posted).toEqual([{
        type: 'resource.bytes',
        id: expect.any(String),
        url: 'data:text/plain;base64,aGk=',
      }]);

      window.dispatchEvent(new MessageEvent('message', {
        data: {
          type: 'resource.bytes.result',
          id: posted[0].id,
          blob: new Blob(['hi'], { type: 'text/plain' }),
          mime: 'text/plain',
        },
      }));

      expect(await (await promise).text()).toBe('hi');
    } finally {
      Object.defineProperty(window, 'parent', { configurable: true, value: previousParent });
      delete (window as unknown as { napplet?: unknown }).napplet;
    }
  });
});
