# Agent Note: Eliminate synthetic stop chunks on empty abruptly ended streams

Status: implemented

## Problem

When upstream LLM streams terminate prematurely or abruptly without emitting any valid candidate frames or content tokens:
1. `antigravity_sse_to_openai_stream` in `crates/ponyllm-server/src/streaming.rs` historically synthesized a fallback terminal chunk with `finish_reason: "stop"` whenever `!stopped` and `!transport_errored`.
2. If upstream closed the connection or returned an empty stream without ever producing a single chunk or content delta, synthesizing a `finish_reason: "stop"` falsely indicates to downstream clients that the model successfully completed its generation.
3. This creates a protocol semantic flaw: downstream clients (such as `deepseek-harness` or Claude Code) receive an empty response marked with a normal `stop` finish reason, masking the abnormal termination or empty upstream stream as a normal completion.

## Decision

We refine the terminal stream chain behavior in `crates/ponyllm-server/src/streaming.rs`:
1. Track whether upstream has actually emitted any candidate or response chunks (`has_emitted_chunks`).
2. Only synthesize a graceful `finish_reason: "stop"` chunk if the stream actually emitted at least one response chunk prior to EOF (i.e. generation started and flowed, but the upstream omitted an explicit terminal choice STOP).
3. If the stream ended without emitting any chunks or content (e.g. abrupt EOF from upstream with 0 tokens/candidates), **do not synthesize a fake `stop` chunk**. The stream terminates directly with `data: [DONE]` or drops cleanly, allowing downstream clients to accurately recognize that zero generation occurred and trigger their native retry / error recovery policies.
4. Update unit tests to verify this contract.

## Alternatives considered

- **Synthesize an OpenAI error JSON event on empty stream before `[DONE]`:**
  Rejected because standard OpenAI chat completion SSE streams do not define mid-stream error schemas uniformly across all clients, and many clients fail to parse non-standard error chunks inside SSE chunks.
- **Keep synthesizing fake Stop chunk unconditionally:**
  Rejected because it presents a false successful completion to downstream agents, turning upstream connection anomalies into degenerate empty completions.

## Consequences

- If upstream silently terminates with zero chunks, downstream clients will no longer receive a fake `stop` chunk.
- For legitimate multi-chunk streams that merely missed an explicit stop flag, the graceful Stop fallback remains active.
- Telemetry and downstream error handlers can distinguish between completed requests and abrupt zero-content closures.
