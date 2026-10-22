import { describe, expect, it } from 'vitest';
import { SseParser } from './sse';

describe('SseParser', () => {
  it('reads named events split across chunks', () => {
    const p = new SseParser();
    expect(p.push('event: changed\ndata: {"blo')).toEqual([]);
    expect(p.push('cks":[3]}\n\nevent: focus\n')).toEqual([{ event: 'changed', data: '{"blocks":[3]}' }]);
    expect(p.push('data: {"block":null}\n\n')).toEqual([{ event: 'focus', data: '{"block":null}' }]);
  });

  it('ignores keep-alive comments and joins multi-line data', () => {
    const p = new SseParser();
    expect(p.push(':\n\n: ping\n\ndata: a\ndata: b\n\n')).toEqual([{ event: 'message', data: 'a\nb' }]);
  });

  it('accepts CRLF line endings', () => {
    const p = new SseParser();
    expect(p.push('event: theme\r\ndata: {}\r\n\r\n')).toEqual([{ event: 'theme', data: '{}' }]);
  });
});
