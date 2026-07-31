import { afterEach, describe, expect, it, vi } from 'vitest';

const { postToShell } = vi.hoisted(() => ({ postToShell: vi.fn() }));
vi.mock('../boundary.js', () => ({ postToShell }));

import {
  installListsShim,
  LISTS_PENDING_REQUEST_LIMIT,
  supported,
} from './shim.js';

describe('lists shim pending-request admission', () => {
  const cleanup = installListsShim();

  afterEach(() => {
    cleanup();
    postToShell.mockClear();
  });

  it('immediately refuses global overflow without posting or retaining it', async () => {
    const admitted = Array.from(
      { length: LISTS_PENDING_REQUEST_LIMIT },
      () => supported(),
    );
    const settled = Promise.allSettled(admitted);

    await expect(supported()).rejects.toThrow(
      `lists request capacity reached (${LISTS_PENDING_REQUEST_LIMIT} pending)`,
    );
    expect(postToShell).toHaveBeenCalledTimes(LISTS_PENDING_REQUEST_LIMIT);

    cleanup();
    await settled;
  });
});
