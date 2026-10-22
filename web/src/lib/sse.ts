// A small server-sent events parser. The page reads the event stream with fetch()
// rather than EventSource, because EventSource can't send the write key header.

export interface SseMessage {
  event: string;
  data: string;
}

/** Feed it text as it arrives; it returns each complete message. */
export class SseParser {
  #buffer = '';
  #event = '';
  #data: string[] = [];

  push(chunk: string): SseMessage[] {
    this.#buffer += chunk;
    const out: SseMessage[] = [];
    let nl = this.#buffer.indexOf('\n');
    while (nl >= 0) {
      let line = this.#buffer.slice(0, nl);
      this.#buffer = this.#buffer.slice(nl + 1);
      if (line.endsWith('\r')) line = line.slice(0, -1);
      this.#line(line, out);
      nl = this.#buffer.indexOf('\n');
    }
    return out;
  }

  #line(line: string, out: SseMessage[]): void {
    if (line === '') {
      if (this.#data.length > 0) out.push({ event: this.#event || 'message', data: this.#data.join('\n') });
      this.#event = '';
      this.#data = [];
      return;
    }
    if (line.startsWith(':')) return; // a keep-alive comment
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') this.#event = value;
    else if (field === 'data') this.#data.push(value);
  }
}
