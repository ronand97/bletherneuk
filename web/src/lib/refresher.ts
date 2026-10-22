// Coalesces reloads: bursts of `changed` events turn into one fetch, and a request that
// arrives while a fetch is running causes exactly one more fetch after it.

export interface Refresher {
  /** Asks for a reload soon. Several calls in a short time cause one reload. */
  request(): void;
  /** Reloads now (or right after the running reload) and resolves when done. */
  now(): Promise<void>;
  dispose(): void;
}

export function createRefresher(run: () => Promise<void>, delayMs = 150): Refresher {
  let running: Promise<void> | null = null;
  let pending = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let disposed = false;

  async function loop(): Promise<void> {
    do {
      pending = false;
      try {
        await run();
      } catch {
        // Loaders report their own errors.
      }
    } while (pending && !disposed);
  }

  function start(): Promise<void> {
    if (running) {
      pending = true;
      return running;
    }
    running = loop().finally(() => {
      running = null;
    });
    return running;
  }

  return {
    request() {
      if (disposed) return;
      if (running) {
        pending = true;
        return;
      }
      if (timer) return;
      timer = setTimeout(() => {
        timer = null;
        void start();
      }, delayMs);
    },
    now() {
      if (disposed) return Promise.resolve();
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
      return start();
    },
    dispose() {
      disposed = true;
      if (timer) clearTimeout(timer);
      timer = null;
    },
  };
}
