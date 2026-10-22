// Busy and error state for one button or form, so `{"ok": false, "error": ...}` replies
// show inline, as text, next to what was clicked.

import { post } from './api';

export class ActionState {
  busy = $state(false);
  error = $state<string | null>(null);

  async run(path: string, body: unknown = {}): Promise<boolean> {
    if (this.busy) return false;
    this.busy = true;
    this.error = null;
    const result = await post(path, body);
    this.busy = false;
    if (!result.ok) this.error = result.error;
    return result.ok;
  }

  fail(message: string): void {
    this.error = message;
  }

  clear(): void {
    this.error = null;
  }
}
