import { describe, it, expect } from 'vitest';
import { scrubSecrets, maskKey, generateCurlCommand } from './scrub';
import type { RecordedFrame } from '../types/telemetry';

describe('scrubSecrets & maskKey (WEB-02 security compliance)', () => {
  it('scrubs raw openai-style keys in free text', () => {
    const raw = 'Failed with authorization header Bearer sk-1234567890abcdef1234567890abcdef and code 401';
    const cleaned = scrubSecrets(raw);
    expect(cleaned).not.toContain('1234567890abcdef');
    expect(cleaned).toBe('Failed with authorization header Bearer sk-*** and code 401');
  });

  it('scrubs keys in json snippet strings', () => {
    const jsonSnippet = '{"model":"gpt-4","api_key":"sk-proj-abc123xyz890_secretKey"}';
    const cleaned = scrubSecrets(jsonSnippet);
    expect(cleaned).toBe('{"model":"gpt-4","api_key":"sk-***"}');
  });

  it('normalizes partially masked backend keys to sk-***', () => {
    expect(maskKey('sk-***abcd')).toBe('sk-***');
    expect(maskKey('sk-1234567890abcdef')).toBe('sk-***');
    expect(maskKey('sk-proj-abcdef123456')).toBe('sk-***');
    expect(maskKey('****')).toBe('****');
    expect(maskKey('')).toBe('');
  });

  it('leaves clean text untouched', () => {
    const clean = '{"message":"hello world","code":200}';
    expect(scrubSecrets(clean)).toBe(clean);
  });
});

describe('generateCurlCommand (WEB-02 reproduction & injection safety)', () => {
  const sampleFrame: RecordedFrame = {
    request_id: 'req-123',
    timestamp: '2026-09-07T12:00:00Z',
    endpoint: '/v1/chat/completions',
    provider: 'deepseek',
    key_id: 'key-test',
    sanitized_key: 'sk-***abcd',
    status_code: 200,
    latency_ms: 125,
    request_snippet: '{"messages":[{"role":"user","content":"say \'hi\' & exit"}]}',
  };

  it('generates curl with masked sk-*** authorization header', () => {
    const cmd = generateCurlCommand(sampleFrame, 'http://127.0.0.1:8080');
    expect(cmd).toContain("curl -X POST 'http://127.0.0.1:8080/v1/chat/completions'");
    expect(cmd).toContain("-H 'Authorization: Bearer sk-***'");
    expect(cmd).not.toContain('sk-***abcd');
  });

  it('safely escapes single quotes in request payload to prevent shell injection', () => {
    const cmd = generateCurlCommand(sampleFrame);
    // single quote in payload "say 'hi'" should be escaped as '\''
    expect(cmd).toContain("'\\''hi'\\''");
  });
});
