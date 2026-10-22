// A shared "now" that ticks, so "3m ago" labels stay current.

class Clock {
  now = $state(Date.now());
  #timer: ReturnType<typeof setInterval> | null = null;

  start(intervalMs = 20_000): void {
    if (this.#timer) return;
    this.#timer = setInterval(() => {
      this.now = Date.now();
    }, intervalMs);
  }
}

export const clock = new Clock();
