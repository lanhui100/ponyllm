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
        }
    }

    pub fn process_event(&mut self, event: MessageStreamEvent) -> Result<Vec<ChatCompletionChunk>> {
        let mut chunks = Vec::new();

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
                let finish_reason = delta.stop_reason.map(|r| match r {
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
            }
            MessageStreamEvent::MessageStop => {}
            _ => {}
        }

        Ok(chunks)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ChatActiveBlock {
    Text,
    Thinking,
    ToolUse(u32),
}

/// FSM to translate streaming OpenAI ChatCompletionChunks into Anthropic MessageStreamEvents
#[derive(Debug, Clone)]
pub struct ChatStreamToAnthropicFsm {
    message_id: String,
    model: String,
    block_counter: u32,
    active_block: Option<ChatActiveBlock>,
    sent_start: bool,
}

impl ChatStreamToAnthropicFsm {
    pub fn new(fallback_model: &str) -> Self {
        Self {
            message_id: format!("msg_{}", uuid_simple()),
            model: fallback_model.to_string(),
            block_counter: 0,
            active_block: None,
            sent_start: false,
        }
    }

    pub fn process_chunk(&mut self, chunk: ChatCompletionChunk) -> Result<Vec<MessageStreamEvent>> {
        let mut events = Vec::new();

        if !self.sent_start {
            self.message_id = chunk.id.clone();
            self.model = chunk.model.clone();
            self.sent_start = true;

            events.push(MessageStreamEvent::MessageStart {
                message: MessageResponse {
                    id: self.message_id.clone(),
                    r#type: "message".to_string(),
                    role: "assistant".to_string(),
                    content: vec![],
                    model: self.model.clone(),
                    stop_reason: None,
                    stop_sequence: None,
                    usage: AnthropicUsage::default(),
                },
            });
        }

        for choice in &chunk.choices {
            // 1. Thinking / Reasoning delta
            if let Some(ref reasoning) = choice.delta.reasoning_content {
                if self.active_block != Some(ChatActiveBlock::Thinking) {
                    if self.active_block.is_some() {
                        events.push(MessageStreamEvent::ContentBlockStop {
                            index: self.block_counter,
                        });
                        self.block_counter += 1;
                    }
                    events.push(MessageStreamEvent::ContentBlockStart {
                        index: self.block_counter,
                        content_block: AnthropicContentBlock::Thinking {
                            thinking: String::new(),
                            signature: None,
                        },
                    });
                    self.active_block = Some(ChatActiveBlock::Thinking);
                }

                events.push(MessageStreamEvent::ContentBlockDelta {
                    index: self.block_counter,
                    delta: AnthropicDelta::ThinkingDelta {
                        thinking: reasoning.clone(),
                    },
                });
            }

            // 2. Text Content delta
            if let Some(ref content) = choice.delta.content {
                if self.active_block != Some(ChatActiveBlock::Text) {
                    if self.active_block.is_some() {
                        events.push(MessageStreamEvent::ContentBlockStop {
                            index: self.block_counter,
                        });
                        self.block_counter += 1;
                    }
                    events.push(MessageStreamEvent::ContentBlockStart {
                        index: self.block_counter,
                        content_block: AnthropicContentBlock::Text {
                            text: String::new(),
                            cache_control: None,
                        },
                    });
                    self.active_block = Some(ChatActiveBlock::Text);
                }

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
                        if self.active_block.is_some() {
                            events.push(MessageStreamEvent::ContentBlockStop {
                                index: self.block_counter,
                            });
                            self.block_counter += 1;
                        }

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

            // 4. Finish reason
            if let Some(ref finish) = choice.finish_reason {
                if self.active_block.is_some() {
                    events.push(MessageStreamEvent::ContentBlockStop {
                        index: self.block_counter,
                    });
                    self.active_block = None;
                }

                let stop_reason = match finish {
                    FinishReason::Stop => Some(AnthropicStopReason::EndTurn),
                    FinishReason::Length => Some(AnthropicStopReason::MaxTokens),
                    FinishReason::ToolCalls | FinishReason::FunctionCall => {
                        Some(AnthropicStopReason::ToolUse)
                    }
                    _ => Some(AnthropicStopReason::Other),
                };

                let output_tokens = chunk.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0);
                events.push(MessageStreamEvent::MessageDelta {
                    delta: MessageDeltaBody {
                        stop_reason,
                        stop_sequence: None,
                    },
                    usage: Some(AnthropicDeltaUsage { output_tokens }),
                });
                events.push(MessageStreamEvent::MessageStop);
            }
        }

        Ok(events)
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
