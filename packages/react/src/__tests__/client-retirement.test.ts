import { afterEach, expect, it, vi } from 'vitest';
import { DarshJDB } from '@darshjdb/client';
import { createDarshanClient } from '../create-client';

afterEach(() => vi.restoreAllMocks());

it('passes retirement errors and cleared data through the React client adapter', async () => {
  vi.spyOn(DarshJDB.prototype, 'send').mockResolvedValue({
    type: 'sub-ok', id: 'request', sub_id: 'subscription', initial: [{ id: 'private-row' }],
  });
  const client = createDarshanClient({ serverUrl: 'https://db.example.com', appId: 'test' });
  const callback = vi.fn();
  const stop = client.subscribe({ collection: 'notes' }, callback);
  await Promise.resolve();
  expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'private-row' }]);
  client.disconnect();
  expect(callback.mock.lastCall?.[0].data).toEqual([]);
  expect(callback.mock.lastCall?.[0].error.message).toContain('reconnect and subscribe again');
  stop();
});
