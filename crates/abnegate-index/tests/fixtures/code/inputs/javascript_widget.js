import { EventEmitter } from 'events';
import fetchJson from './fetch-json.js';

const DEFAULT_TIMEOUT = 2500;

/**
 * Polls a status endpoint and emits changes.
 */
export class StatusPoller extends EventEmitter {
  constructor(url, interval = 1000) {
    super();
    this.url = url;
    this.interval = interval;
    this.timer = null;
    this.lastStatus = null;
  }

  start() {
    if (this.timer) {
      return;
    }
    this.timer = setInterval(() => this.poll(), this.interval);
  }

  stop() {
    clearInterval(this.timer);
    this.timer = null;
  }

  async poll() {
    const body = await fetchJson(this.url, { timeout: DEFAULT_TIMEOUT });
    if (body.status !== this.lastStatus) {
      this.lastStatus = body.status;
      this.emit('change', body.status);
    }
  }
}

function formatStatus(status) {
  return `status: ${status ?? 'unknown'}`;
}

const describeStatus = (status) => formatStatus(status).toUpperCase();

export { formatStatus, describeStatus };

describe('formatStatus', () => {
  it('falls back to unknown', () => {
    expect(formatStatus(undefined)).toBe('status: unknown');
  });
});
