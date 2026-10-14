use std::time::{SystemTime, UNIX_EPOCH};
use crate::anthropic::messages::*;
use crate::error::Result;
use crate::openai::chat::*;

#[derive(Debug, Clone)]
enum CurrentAnthropicBlock {
    Text,
    Thinking,
    ToolUse {
        index: u32,
        #[allow(dead_code)]
        id: String,
        #[allow(dead_code)]
        name: String,
    },
}

/// FSM to translate streaming Anthropic MessageStreamEvents into OpenAI ChatCompletionChunks
#[derive(Debug, Clone)]
pub struct AnthropicStreamToChatFsm {
    response_id: String,
    model: String,
    created: u64,
    current_block: Option<CurrentAnthropicBlock>,
    tool_counter: u32,
    done: bool,
}

impl AnthropicStreamToChatFsm {
    pub fn new(fallback_model: &str) -> Self {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            response_id: format!("chatcmpl-{}", created),
            model: fallback_model.to_string(),
            created,
            current_block: None,
            tool_counter: 0,
            done: false,
        }
    }

    pub fn process_event(&mut self, event: MessageStreamEvent) -> Result<Vec<ChatCompletionChunk>> {
        let mut chunks = Vec::new();
        if self.done {
            return Ok(chunks);
        }

        match event {
            MessageStreamEvent::MessageStart { message } => {
                self.response_id = message.id.clone();
                self.model = message.model.clone();
                chunks.push(ChatCompletionChunk {
                    id: self.response_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChatChunkChoice {
                        index: 0,
                        delta: ChatChunkDelta {
                            role: Some("assistant".to_string()),
                            content: None,
                            reasoning_content: None,
                            refusal: None,
                            tool_calls: None,
                        },
                        finish_reason: None,
                        logprobs: None,
                    }],
                    usage: None,
                    system_fingerprint: None,
                    service_tier: None,
                });
            }
            MessageStreamEvent::ContentBlockStart { index: _, content_block } => {
                match content_block {
                    AnthropicContentBlock::Thinking { .. } => {
                        self.current_block = Some(CurrentAnthropicBlock::Thinking);
                    }
                    AnthropicContentBlock::Text { .. } => {
                        self.current_block = Some(CurrentAnthropicBlock::Text);
                    }
                    AnthropicContentBlock::ToolUse { id, name, .. } => {
                        let idx = self.tool_counter;
                        self.tool_counter += 1;
                        self.current_block = Some(CurrentAnthropicBlock::ToolUse {
                            index: idx,
                            id: id.clone(),
                            name: name.clone(),
                        });

                        chunks.push(ChatCompletionChunk {
                            id: self.response_id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created: self.created,
                            model: self.model.clone(),
                            choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatChunkDelta {
                                    role: None,
                                    content: None,
                                    reasoning_content: None,
                                    refusal: None,
                                    tool_calls: Some(vec![ToolCallChunk {
                                        index: idx,
                                        id: Some(id),
                                        r#type: Some("function".to_string()),
                                        function: Some(FunctionCallChunk {
                                            name: Some(name),
                                            arguments: Some(String::new()),
                                        }),
                                    }]),
                                },
                                finish_reason: None,
                                logprobs: None,
                            }],
                            usage: None,
                            system_fingerprint: None,
                            service_tier: None,
                        });
                    }
                    _ => {}
                }
            }
            MessageStreamEvent::ContentBlockDelta { index: _, delta } => {
                match delta {
                    AnthropicDelta::TextDelta { text } => {
                        chunks.push(ChatCompletionChunk {
                            id: self.response_id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created: self.created,
                            model: self.model.clone(),
                            choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatChunkDelta {
                                    role: None,
                                    content: Some(text),
                                    reasoning_content: None,
                                    refusal: None,
                                    tool_calls: None,
                                },
                                finish_reason: None,
                                logprobs: None,
                            }],
                            usage: None,
                            system_fingerprint: None,
                            service_tier: None,
                        });
                    }
                    AnthropicDelta::ThinkingDelta { thinking } => {
                        chunks.push(ChatCompletionChunk {
                            id: self.response_id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created: self.created,
                            model: self.model.clone(),
                            choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatChunkDelta {
                                    role: None,
                                    content: None,
                                    reasoning_content: Some(thinking),
                                    refusal: None,
                                    tool_calls: None,
                                },
                                finish_reason: None,
                                logprobs: None,
                            }],
                            usage: None,
                            system_fingerprint: None,
                            service_tier: None,
                        });
                    }
                    AnthropicDelta::InputJsonDelta { partial_json } => {
                        if let Some(CurrentAnthropicBlock::ToolUse { index, .. }) = self.current_block {
                            chunks.push(ChatCompletionChunk {
                                id: self.response_id.clone(),
                                object: "chat.completion.chunk".to_string(),
                                created: self.created,
                                model: self.model.clone(),
                                choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatChunkDelta {
                                    role: None,
                                    content: None,
                                    reasoning_content: None,
                                    refusal: None,
                                    tool_calls: Some(vec![ToolCallChunk {
                                        index,
                                        id: None,
                                        r#type: None,
                                        function: Some(FunctionCallChunk {
                                            name: None,
                                            arguments: Some(partial_json),
                                        }),
                                    }]),
                                },
                                finish_reason: None,
                                logprobs: None,
                            }],
                            usage: None,
                            system_fingerprint: None,
                            service_tier: None,
                        });
                        }
                    }
                    _ => {}
                }
            }
            MessageStreamEvent::ContentBlockStop { .. } => {
                self.current_block = None;
            }
            MessageStreamEvent::MessageDelta { delta, usage } => {
                let finish_reason = delta.stop_reason.as_ref().map(|r| match r {
                    AnthropicStopReason::EndTurn => FinishReason::Stop,
                    AnthropicStopReason::MaxTokens => FinishReason::Length,
                    AnthropicStopReason::ToolUse => FinishReason::ToolCalls,
                    AnthropicStopReason::StopSequence => FinishReason::Stop,
                    AnthropicStopReason::Other => FinishReason::Other,
                });

                let chat_usage = usage.map(|u| Usage {
                    prompt_tokens: 0,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.output_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                });

                chunks.push(ChatCompletionChunk {
                    id: self.response_id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChatChunkChoice {
                        index: 0,
                        delta: ChatChunkDelta::default(),
                        finish_reason,
                        logprobs: None,
                    }],
                    usage: chat_usage,
                    system_fingerprint: None,
                    service_tier: None,
                });
                if delta.stop_reason.is_some() {
                    self.done = true;
                }
            }
            MessageStreamEvent::MessageStop => {
                self.done = true;
            }
            _ => {}
        }

        Ok(chunks)
    }

    pub fn finish_if_open(&mut self) -> Option<ChatCompletionChunk> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(ChatCompletionChunk {
            id: self.response_id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatChunkDelta::default(),
                finish_reason: Some(if self.tool_counter > 0 {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }),
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
            service_tier: None,
        })
    }
}


#[derive(Debug, Clone, PartialEq)]
enum ChatActiveBlock {
    Text,
    Thinking,
    ToolUse(u32),
}

/// FSM to translate streaming OpenAI ChatCompletionChunks into Anthropic MessageStreamEvents.
///
/// Contract A-SS-07: exactly one `content_block_start` / `content_block_stop` pair per *logical*
/// content block, with a stable `index` equal to the block's position in the final `content`
/// array. Deltas accumulate into the open block; an empty delta never opens a block and never
/// reaches the wire (A-SS-15). A thinking block is closed with exactly one `signature_delta`
/// immediately before its `content_block_stop` (A-SS-09, value `""` per contract v2 §8.6).
#[derive(Debug, Clone)]
pub struct ChatStreamToAnthropicFsm {
    message_id: String,
    model: String,
    block_counter: u32,
    active_block: Option<ChatActiveBlock>,
    sent_start: bool,
    done: bool,
    /// Cumulative upstream usage (chat-pivot `Usage`), absorbed whenever a chunk carries it —
    /// including the trailing usage-only chunk emitted by `stream_options.include_usage` after
    /// the `finish_reason` chunk (contract §8.4). `usage_seen` distinguishes "upstream reported
    /// zero" from "upstream reported nothing".
    usage_seen: bool,
    prompt_tokens: u32,
    output_tokens: u32,
    cached_tokens: Option<u32>,
    /// `finish_reason` observed before upstream usage arrived: the terminal events are withheld
    /// until the trailing usage-only chunk (or EOF) so `message_delta.usage` can carry the real
    /// cumulative counts (A-SS-11 / A-SS-17).
    pending_finish: Option<AnthropicStopReason>,
}

impl ChatStreamToAnthropicFsm {
    pub fn new(fallback_model: &str) -> Self {
        Self {
            message_id: format!("msg_{}", uuid_simple()),
            model: fallback_model.to_string(),
            block_counter: 0,
            active_block: None,
            sent_start: false,
            done: false,
            usage_seen: false,
            prompt_tokens: 0,
            output_tokens: 0,
            cached_tokens: None,
            pending_finish: None,
        }
    }

    /// A-SS-01..A-SS-06: `message_start` is emitted on the first upstream chunk and MUST NOT be
    /// deferred to await usage (§8.4.2). The real prompt count is unknowable at that instant, so
    /// `input_tokens` is `0` (A-SS-02, relaxed in v2) and the real cumulative counts are carried
    /// on `message_delta.usage` (A-SS-11 / A-SS-17).
    fn message_start_event(&self) -> MessageStreamEvent {
        MessageStreamEvent::MessageStart {
            message: MessageResponse {
                id: self.message_id.clone(),
                r#type: "message".to_string(),
                role: "assistant".to_string(),
                content: vec![],
                model: self.model.clone(),
                stop_reason: None,
                stop_sequence: None,
                usage: AnthropicUsage {
                    input_tokens: 0,
                    // A-SS-03: every official streaming example shows a non-zero
                    // `output_tokens` in `message_start`; the conventional placeholder is 1.
                    output_tokens: 1,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            },
        }
    }

    /// Close the currently open block. A-SS-09: the single `signature_delta` is emitted
    /// immediately before the thinking block's `content_block_stop`.
    fn close_active_block(&mut self, events: &mut Vec<MessageStreamEvent>) {
        if let Some(block) = self.active_block.take() {
            if matches!(block, ChatActiveBlock::Thinking) {
                events.push(MessageStreamEvent::ContentBlockDelta {
                    index: self.block_counter,
                    delta: AnthropicDelta::SignatureDelta {
                        signature: String::new(),
                    },
                });
            }
            events.push(MessageStreamEvent::ContentBlockStop {
                index: self.block_counter,
            });
            self.block_counter += 1;
        }
    }

    /// Open `kind` if it is not already the active block, closing the previous one first so the
    /// `index` sequence stays contiguous and equals the block's position in the final content
    /// array (A-SS-07).
    fn ensure_block(&mut self, kind: ChatActiveBlock, events: &mut Vec<MessageStreamEvent>) {
        if self.active_block.as_ref() == Some(&kind) {
            return;
        }
        let content_block = if matches!(kind, ChatActiveBlock::Thinking) {
            AnthropicContentBlock::Thinking {
                thinking: String::new(),
                signature: None,
            }
        } else if matches!(kind, ChatActiveBlock::Text) {
            AnthropicContentBlock::Text {
                text: String::new(),
                cache_control: None,
            }
        } else {
            // Tool blocks carry an id/name and are opened by the tool-call branch below.
            return;
        };
        self.close_active_block(events);
        events.push(MessageStreamEvent::ContentBlockStart {
            index: self.block_counter,
            content_block,
        });
        self.active_block = Some(kind);
    }

    /// A-SS-10 / A-SS-11 / A-SS-17 / A-SS-19 / A-SS-20: the terminal `message_delta` with the real
    /// cumulative counts. On a path where the upstream never reported usage the honest value `0`
    /// is emitted (Lead ruling) — never a fabricated non-zero.
    fn message_delta_event(&self, stop_reason: AnthropicStopReason) -> MessageStreamEvent {
        MessageStreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(stop_reason),
                stop_sequence: None,
            },
            usage: Some(AnthropicDeltaUsage {
                output_tokens: self.output_tokens,
                input_tokens: if self.usage_seen {
                    Some(self.prompt_tokens)
                } else {
                    None
                },
                cache_read_input_tokens: if self.usage_seen {
                    self.cached_tokens
                } else {
                    None
                },
                ..Default::default()
            }),
        }
    }

    fn finish_turn(
        &mut self,
        stop_reason: AnthropicStopReason,
        events: &mut Vec<MessageStreamEvent>,
    ) {
        self.close_active_block(events);
        events.push(self.message_delta_event(stop_reason));
        events.push(MessageStreamEvent::MessageStop);
        self.done = true;
        self.pending_finish = None;
    }

    pub fn process_chunk(&mut self, chunk: ChatCompletionChunk) -> Result<Vec<MessageStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }

        if !self.sent_start {
            // A-SS-06: `msg_`-prefixed, per-request-distinct id derived from the upstream id.
            self.message_id = msg_id_from(&chunk.id, &self.message_id);
            self.model = chunk.model.clone();
            self.sent_start = true;
            events.push(self.message_start_event());
        }

        // §8.4: absorb upstream usage wherever it appears. The `include_usage` shape delivers it
        // in a trailing chunk with `choices: []` after the `finish_reason` chunk.
        if let Some(u) = chunk.usage.as_ref() {
            self.usage_seen = true;
            self.prompt_tokens = u.prompt_tokens;
            self.output_tokens = u.completion_tokens;
            self.cached_tokens = u
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens);
        }

        if self.pending_finish.is_none() {
            for choice in &chunk.choices {
                // A-SS-12: the terminal set is emitted exactly once and last. A chunk may carry
                // several choices, so re-check `done` between them: once `finish_turn()` has run,
                // no further choice may emit anything.
                if self.done {
                    break;
                }

                // 1. Thinking / reasoning delta. A-SS-07/A-SS-15: an empty delta neither opens a
                //    block nor reaches the wire.
                if let Some(reasoning) = choice
                    .delta
                    .reasoning_content
                    .as_ref()
                    .filter(|r| !r.is_empty())
                {
                    self.ensure_block(ChatActiveBlock::Thinking, &mut events);
                    events.push(MessageStreamEvent::ContentBlockDelta {
                        index: self.block_counter,
                        delta: AnthropicDelta::ThinkingDelta {
                            thinking: reasoning.clone(),
                        },
                    });
                }

                // 2. Text content delta
                if let Some(content) = choice.delta.content.as_ref().filter(|c| !c.is_empty()) {
                    self.ensure_block(ChatActiveBlock::Text, &mut events);
                    events.push(MessageStreamEvent::ContentBlockDelta {
                        index: self.block_counter,
                        delta: AnthropicDelta::TextDelta {
                            text: content.clone(),
                        },
                    });
                }

                // 3. Tool calls delta
                if let Some(ref tool_calls) = choice.delta.tool_calls {
                    for tc in tool_calls {
                        let tool_idx = tc.index;
                        if self.active_block != Some(ChatActiveBlock::ToolUse(tool_idx)) {
                            self.close_active_block(&mut events);

                            let id = tc.id.clone().unwrap_or_else(|| format!("call_{}", tool_idx));
                            let name = tc
                                .function
                                .as_ref()
                                .and_then(|f| f.name.clone())
                                .unwrap_or_default();

                            events.push(MessageStreamEvent::ContentBlockStart {
                                index: self.block_counter,
                                content_block: AnthropicContentBlock::ToolUse {
                                    id,
                                    name,
                                    input: serde_json::json!({}),
                                    cache_control: None,
                                },
                            });
                            self.active_block = Some(ChatActiveBlock::ToolUse(tool_idx));
                        }

                        if let Some(ref func) = tc.function {
                            if let Some(ref args) = func.arguments {
                                if !args.is_empty() {
                                    events.push(MessageStreamEvent::ContentBlockDelta {
                                        index: self.block_counter,
                                        delta: AnthropicDelta::InputJsonDelta {
                                            partial_json: args.clone(),
                                        },
                                    });
                                }
                            }
                        }
                    }
                }

                // 4. Finish reason. The chat pivot cannot distinguish a custom stop-sequence
                //    match (`FinishReason` has no `StopSequence` variant), so it projects to
                //    `end_turn`; `ContentFilter`/`Other` project there too (Lead ruling).
                if let Some(ref finish) = choice.finish_reason {
                    let stop_reason = match finish {
                        FinishReason::Length => AnthropicStopReason::MaxTokens,
                        FinishReason::ToolCalls | FinishReason::FunctionCall => {
                            AnthropicStopReason::ToolUse
                        }
                        _ => AnthropicStopReason::EndTurn,
                    };

                    if self.usage_seen {
                        // Usage already known: emit the full terminal set now.
                        self.finish_turn(stop_reason, &mut events);
                    } else {
                        // Withhold the terminal set for the trailing usage-only chunk (§8.4).
                        self.pending_finish = Some(stop_reason);
                    }
                }
            }
        }

        // §8.4: a trailing usage-only chunk (`choices: []`, usage present) completes a turn whose
        // `finish_reason` chunk carried no usage. It must not surface as content.
        if !self.done && self.usage_seen {
            if let Some(stop_reason) = self.pending_finish.take() {
                self.finish_turn(stop_reason, &mut events);
            }
        }

        Ok(events)
    }

    pub fn finish_if_open(&mut self) -> Option<Vec<MessageStreamEvent>> {
        if self.done {
            return None;
        }
        self.done = true;
        let mut events = Vec::new();
        if !self.sent_start {
            self.sent_start = true;
            events.push(self.message_start_event());
        }
        let stop_reason = self
            .pending_finish
            .take()
            .unwrap_or(AnthropicStopReason::EndTurn);
        self.close_active_block(&mut events);
        events.push(self.message_delta_event(stop_reason));
        events.push(MessageStreamEvent::MessageStop);
        Some(events)
    }
}

/// A-SS-06: derive a `msg_`-prefixed, per-request-distinct id from the upstream chat completion
/// id. Falls back to the FSM's own time-based id when the upstream id has no alphanumerics.
fn msg_id_from(upstream_id: &str, fallback: &str) -> String {
    let alnum: String = upstream_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if alnum.is_empty() {
        fallback.to_string()
    } else {
        format!("msg_{}", alnum)
    }
}

fn uuid_simple() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", now)
}
