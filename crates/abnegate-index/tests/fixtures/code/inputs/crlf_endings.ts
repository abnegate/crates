import { Logger } from "./logger";

export class RetryPolicy {
  constructor(private readonly attempts: number, private readonly delayMs: number) {}

  async run<T>(task: () => Promise<T>, logger: Logger): Promise<T> {
    let lastError: unknown;
    for (let attempt = 1; attempt <= this.attempts; attempt++) {
      try {
        return await task();
      } catch (error) {
        lastError = error;
        logger.warn(`attempt ${attempt} failed`);
        await new Promise((resolve) => setTimeout(resolve, this.delayMs * attempt));
      }
    }
    throw lastError;
  }
}

export function noRetry(): RetryPolicy {
  return new RetryPolicy(1, 0);
}
