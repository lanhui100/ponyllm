import type { RecordedFrame } from '../types/telemetry';

/**
 * Scrubs any occurrence of OpenAI/Anthropic/custom keys starting with `sk-`
 * and replaces them strictly with `sk-***`.
 * Covers raw key text, json values, error messages, and snippets.
 */
export function scrubSecrets(text: string): string {
  if (!text) return '';
  // Match sk- followed by any word chars, hyphens, or masked stars (e.g. sk-***1234 or sk-proj-...)
  return text.replace(/sk-[A-Za-z0-9_*.-]+/g, 'sk-***');
}

/**
 * Normalizes any key string (including backend partially sanitized keys like `sk-***1234`)
 * into the strict display format `sk-***` or `****`.
 */
export function maskKey(key: string): string {
  if (!key) return '';
  if (key.includes('sk-')) {
    return 'sk-***';
  }
  if (key.includes('*')) {
    return key;
  }
  return '****';
}

/**
 * Generates a safe curl command for reproduction from a RecordedFrame.
 * - Always uses a safe placeholder / masked header: `Authorization: Bearer sk-***`.
 * - Safely escapes shell single quotes in request payload to prevent command injection.
 */
export function generateCurlCommand(frame: RecordedFrame, baseUrl?: string): string {
  const host = (baseUrl || '').replace(/\/$/, '');
  const endpoint = frame.endpoint.startsWith('/') ? frame.endpoint : `/${frame.endpoint}`;
  const fullUrl = `${host}${endpoint}`;

  const lines: string[] = [`curl -X POST '${fullUrl}'`];
  lines.push("  -H 'Authorization: Bearer sk-***'");
  lines.push("  -H 'Content-Type: application/json'");

  if (frame.request_snippet) {
    const scrubbed = scrubSecrets(frame.request_snippet);
    // Escape single quotes: ' -> '\''
    const escapedPayload = scrubbed.replace(/'/g, "'\\''");
    lines.push(`  -d '${escapedPayload}'`);
  }

  return lines.join(' \\\n');
}
