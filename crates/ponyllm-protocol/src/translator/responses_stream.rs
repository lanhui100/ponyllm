use crate::anthropic::messages::*;
use crate::error::{ProtocolError, Result};
use crate::openai::chat::*;
use crate::openai::responses::*;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn uuid_simple() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}", now)
}

/// Process-unique suffix for generated response/item ids. A bare nanosecond timestamp is not
/// enough: concurrent requests can be constructed inside the same clock tick, and R-SS-19 requires
/// pairwise-distinct response ids across concurrent streams.
fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}{:x}", uuid_simple(), n)
}

fn chat_text_chunk(id: &str, model: &str, created: u64, text: String) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
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
    }
}

/// Failed Responses objects project to a Conversion error, never to a
/// terminal chat chunk: swallowing failure as `FinishReason::Other` would
/// surface a successful empty response to the client.
fn failed_conversion_error(response: &ResponseObject) -> ProtocolError {
    ProtocolError::Conversion {
        from: "responses",
        to: "chat",
        reason: response.failed_reason(),
    }
}

fn skeleton_response(id: &str, model: &str, created_at: i64, status: &str) -> ResponseObject {
    ResponseObject {
        id: id.to_string(),
        object: "response".to_string(),
        created_at,
        completed_at: None,
        status: status.to_string(),
        model: model.to_string(),
        output: vec![],
        usage: None,
        ..Default::default()
    }
}

/// Shared emitter for the official Responses SSE item lifecycle.
///
/// The chat-native and Anthropic-native upstreams both project into the same wire sequence, so the
/// `response.created` / `response.in_progress` / reasoning-summary / message / content-part
/// lifecycle lives in exactly one place (R-SS-01..R-SS-15, R-SS-23 and the R-SS-21 parity rule).
#[derive(Debug, Clone)]
struct ResponsesEmitter {
    response_id: String,
    model: String,
    /// Stable for the whole stream: one value reused by every event (R-SS-18-style stability).
    created_at: i64,
    seq: u64,
    started: bool,
    reasoning_item_id: String,
    reasoning_acc: String,
    reasoning_index: Option<u32>,
    reasoning_open: bool,
    reasoning_closed: bool,
    message_item_id: String,
    text_acc: String,
    message_index: Option<u32>,
    message_open: bool,
    message_closed: bool,
    next_index: u32,
}

impl ResponsesEmitter {
    fn new(response_id: String, model: &str, created_at: i64) -> Self {
        let tail = response_id
            .strip_prefix("resp_")
            .unwrap_or(&response_id)
            .to_string();
        Self {
            response_id,
            model: model.to_string(),
            created_at,
            seq: 0,
            started: false,
            reasoning_item_id: format!("rs_{tail}"),
            reasoning_acc: String::new(),
            reasoning_index: None,
            reasoning_open: false,
            reasoning_closed: false,
            message_item_id: format!("msg_{tail}"),
            text_acc: String::new(),
            message_index: None,
            message_open: false,
            message_closed: false,
            next_index: 0,
        }
    }

    fn next_seq(&mut self) -> u64 {
        let n = self.seq;
        self.seq += 1;
        n
    }

    /// Allocates the next `output_index` (position in the final `output[]` array).
    fn allocate_index(&mut self) -> u32 {
        let i = self.next_index;
        self.next_index += 1;
        i
    }

    fn skeleton(&self, status: &str) -> ResponseObject {
        skeleton_response(&self.response_id, &self.model, self.created_at, status)
    }

    /// `response.created` + `response.in_progress`, emitted once (R-SS-01/02/03).
    fn ensure_started(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ResponseCreated {
            sequence_number: seq,
            response: self.skeleton("in_progress"),
        });
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ResponseInProgress {
            sequence_number: seq,
            response: self.skeleton("in_progress"),
        });
    }

    fn open_reasoning(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if self.reasoning_open || self.reasoning_closed {
            return;
        }
        let index = self.allocate_index();
        self.reasoning_index = Some(index);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputItemAdded {
            sequence_number: seq,
            response_id: String::new(),
            output_index: index,
            item: ResponseOutputItem::Reasoning {
                id: Some(self.reasoning_item_id.clone()),
                status: Some("in_progress".to_string()),
                encrypted_content: None,
                content: None,
                summary: Some(vec![]),
            },
        });
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ReasoningSummaryPartAdded(
            ResponseReasoningSummaryPart {
                sequence_number: seq,
                item_id: self.reasoning_item_id.clone(),
                output_index: index,
                summary_index: 0,
                part: ResponseContentPart::SummaryText {
                    text: String::new(),
                },
            },
        ));
        self.reasoning_open = true;
    }

    fn reasoning_delta(&mut self, delta: &str, events: &mut Vec<ResponseStreamEvent>) {
        if delta.is_empty() {
            return;
        }
        self.ensure_started(events);
        if self.message_open {
            self.close_message(events);
        }
        self.open_reasoning(events);
        self.reasoning_acc.push_str(delta);
        let index = self.reasoning_index.unwrap_or(0);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ReasoningSummaryTextDelta(
            ResponseReasoningSummaryTextDelta {
                sequence_number: seq,
                item_id: self.reasoning_item_id.clone(),
                output_index: index,
                summary_index: 0,
                delta: delta.to_string(),
            },
        ));
    }

    fn close_reasoning(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if !self.reasoning_open {
            return;
        }
        self.reasoning_open = false;
        self.reasoning_closed = true;
        let index = self.reasoning_index.unwrap_or(0);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ReasoningSummaryTextDone(
            ResponseReasoningSummaryTextDone {
                sequence_number: seq,
                item_id: self.reasoning_item_id.clone(),
                output_index: index,
                summary_index: 0,
                text: self.reasoning_acc.clone(),
            },
        ));
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ReasoningSummaryPartDone(
            ResponseReasoningSummaryPart {
                sequence_number: seq,
                item_id: self.reasoning_item_id.clone(),
                output_index: index,
                summary_index: 0,
                part: ResponseContentPart::SummaryText {
                    text: self.reasoning_acc.clone(),
                },
            },
        ));
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputItemDone {
            sequence_number: seq,
            response_id: String::new(),
            output_index: index,
            item: self.reasoning_item(),
        });
    }

    fn reasoning_item(&self) -> ResponseOutputItem {
        reasoning_output_item(&self.response_id, &self.reasoning_acc)
    }

    fn open_message(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if self.message_open || self.message_closed {
            return;
        }
        let index = self.allocate_index();
        self.message_index = Some(index);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputItemAdded {
            sequence_number: seq,
            response_id: String::new(),
            output_index: index,
            item: ResponseOutputItem::Message {
                id: self.message_item_id.clone(),
                status: "in_progress".to_string(),
                role: "assistant".to_string(),
                content: vec![],
            },
        });
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ContentPartAdded {
            sequence_number: seq,
            response_id: String::new(),
            item_id: self.message_item_id.clone(),
            output_index: index,
            content_index: 0,
            part: ResponseContentPart::Text {
                text: String::new(),
            },
        });
        self.message_open = true;
    }

    fn text_delta(&mut self, delta: &str, events: &mut Vec<ResponseStreamEvent>) {
        if delta.is_empty() {
            return;
        }
        self.ensure_started(events);
        self.close_reasoning(events);
        self.open_message(events);
        self.text_acc.push_str(delta);
        let index = self.message_index.unwrap_or(0);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputTextDelta(ResponseOutputTextDelta {
            sequence_number: seq,
            item_id: self.message_item_id.clone(),
            output_index: index,
            content_index: 0,
            delta: delta.to_string(),
            logprobs: vec![],
        }));
    }

    fn close_message(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if !self.message_open {
            return;
        }
        self.message_open = false;
        self.message_closed = true;
        let index = self.message_index.unwrap_or(0);
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputTextDone(ResponseOutputTextDone {
            sequence_number: seq,
            item_id: self.message_item_id.clone(),
            output_index: index,
            content_index: 0,
            text: self.text_acc.clone(),
            logprobs: vec![],
        }));
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::ContentPartDone {
            sequence_number: seq,
            response_id: String::new(),
            item_id: self.message_item_id.clone(),
            output_index: index,
            content_index: 0,
            part: ResponseContentPart::Text {
                text: self.text_acc.clone(),
            },
        });
        let seq = self.next_seq();
        events.push(ResponseStreamEvent::OutputItemDone {
            sequence_number: seq,
            response_id: String::new(),
            output_index: index,
            item: self.message_item(),
        });
    }

    fn message_item(&self) -> ResponseOutputItem {
        ResponseOutputItem::Message {
            id: self.message_item_id.clone(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![ResponseContentPart::Text {
                text: self.text_acc.clone(),
            }],
        }
    }

    /// `(output_index, item)` for every item this emitter opened, in announcement order.
    fn opened_items(&self) -> Vec<(u32, ResponseOutputItem)> {
        let mut items = Vec::new();
        if self.reasoning_index.is_some() {
            items.push((self.reasoning_index.unwrap_or(0), self.reasoning_item()));
        }
        if self.message_index.is_some() {
            items.push((self.message_index.unwrap_or(0), self.message_item()));
        }
        items
    }

    /// Builds the terminal `response.completed` / `response.incomplete` event (R-SS-12/13/14/15).
    fn terminal_event(
        &mut self,
        finish: &FinishReason,
        usage: Option<&Usage>,
        tool_items: Vec<(u32, ResponseOutputItem)>,
    ) -> ResponseStreamEvent {
        let (input_tokens, output_tokens, cached_tokens, reasoning_tokens) = match usage {
            Some(u) => (
                u.prompt_tokens,
                u.completion_tokens,
                u.prompt_tokens_details
                    .as_ref()
                    .and_then(|d| d.cached_tokens)
                    .unwrap_or(0),
                u.completion_tokens_details
                    .as_ref()
                    .and_then(|d| d.reasoning_tokens)
                    .unwrap_or(0),
            ),
            // No usage reported at all: honest zeros, never a fabricated count.
            None => (0, 0, 0, 0),
        };
        let status = if matches!(finish, FinishReason::Length) {
            "incomplete"
        } else {
            "completed"
        };
        let mut items = self.opened_items();
        items.extend(tool_items);
        items.sort_by_key(|(i, _)| *i);
        let output: Vec<ResponseOutputItem> = items.into_iter().map(|(_, item)| item).collect();
        let response = ResponseObject {
            id: self.response_id.clone(),
            object: "response".to_string(),
            created_at: self.created_at,
            completed_at: Some(now_secs() as i64),
            status: status.to_string(),
            model: self.model.clone(),
            output,
            usage: Some(ResponseUsage::new(
                input_tokens,
                output_tokens,
                cached_tokens,
                reasoning_tokens,
            )),
            ..Default::default()
        };
        let seq = self.next_seq();
        if matches!(finish, FinishReason::Length) {
            ResponseStreamEvent::Incomplete {
                sequence_number: seq,
                response,
            }
        } else {
            ResponseStreamEvent::Completed {
                sequence_number: seq,
                response,
            }
        }
    }

    /// Closes every open item and appends the terminal event. Used on clean EOF and on an upstream
    /// abort, so the client always receives a well-formed terminal frame (R-SS-22).
    fn finish_open(
        &mut self,
        finish: &FinishReason,
        usage: Option<&Usage>,
        tool_items: Vec<(u32, ResponseOutputItem)>,
    ) -> Vec<ResponseStreamEvent> {
        let mut events = Vec::new();
        self.ensure_started(&mut events);
        self.close_reasoning(&mut events);
        self.close_message(&mut events);
        events.push(self.terminal_event(finish, usage, tool_items));
        events
    }
}

/// FSM translating upstream OpenAI Responses SSE into OpenAI Chat chunks.
/// Handles both the legacy `response.text.delta` and the real wire
/// `response.output_text.delta`, plus `response.completed` / `response.done`.
#[derive(Debug, Clone)]
pub struct ResponsesToChatFsm {
    response_id: String,
    model: String,
    created: u64,
    tool_index: HashMap<String, u32>,
    tool_item_to_id: HashMap<String, String>,
    tool_counter: u32,
    saw_tool: bool,
    done: bool,
}

impl ResponsesToChatFsm {
    pub fn new(fallback_model: &str) -> Self {
        Self {
            response_id: format!("chatcmpl-{}", now_secs()),
            model: fallback_model.to_string(),
            created: now_secs(),
            tool_index: HashMap::new(),
            tool_item_to_id: HashMap::new(),
            tool_counter: 0,
            saw_tool: false,
            done: false,
        }
    }

    fn tool_start_chunk(&mut self, call_id: &str, name: Option<String>) -> ChatCompletionChunk {
        let idx = match self.tool_index.get(call_id) {
            Some(i) => *i,
            None => {
                let i = self.tool_counter;
                self.tool_counter += 1;
                self.tool_index.insert(call_id.to_string(), i);
                i
            }
        };
        self.saw_tool = true;
        ChatCompletionChunk {
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
                        id: Some(call_id.to_string()),
                        r#type: Some("function".to_string()),
                        function: Some(FunctionCallChunk {
                            name,
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
        }
    }

    pub fn process_event(
        &mut self,
        event: ResponseStreamEvent,
    ) -> Result<Vec<ChatCompletionChunk>> {
        let mut chunks = Vec::new();
        if self.done {
            return Ok(chunks);
        }
        match event {
            ResponseStreamEvent::ResponseCreated { response, .. } => {
                self.response_id = response.id.clone();
                self.model = response.model.clone();
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
            ResponseStreamEvent::TextDelta(d) => {
                chunks.push(chat_text_chunk(
                    &self.response_id,
                    &self.model,
                    self.created,
                    d.delta,
                ));
            }
            ResponseStreamEvent::OutputTextDelta(d) => {
                chunks.push(chat_text_chunk(
                    &self.response_id,
                    &self.model,
                    self.created,
                    d.delta,
                ));
            }
            ResponseStreamEvent::ReasoningTextDelta(d) => {
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
                            reasoning_content: Some(d.delta),
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
            ResponseStreamEvent::ReasoningSummaryTextDelta(d) => {
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
                            reasoning_content: Some(d.delta),
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
            ResponseStreamEvent::OutputItemAdded {
                item: ResponseOutputItem::FunctionCall { id, call_id, name, .. },
                ..
            } => {
                let effective_call_id = if !call_id.is_empty() {
                    call_id
                } else if !id.is_empty() {
                    id.clone()
                } else {
                    format!("call_{}", self.tool_counter)
                };
                if !id.is_empty() {
                    self.tool_item_to_id.insert(id, effective_call_id.clone());
                }
                chunks.push(self.tool_start_chunk(&effective_call_id, Some(name)));
            }
            ResponseStreamEvent::FunctionCallArgumentsDelta(d) => {
                let resolved_id = if !d.call_id.is_empty() {
                    d.call_id.clone()
                } else if let Some(mapped) = self.tool_item_to_id.get(&d.item_id) {
                    mapped.clone()
                } else if !d.item_id.is_empty() {
                    d.item_id.clone()
                } else {
                    String::new()
                };
                let idx = match self.tool_index.get(&resolved_id) {
                    Some(i) => *i,
                    None => {
                        let start = self.tool_start_chunk(&resolved_id, None);
                        let i = self.tool_index[&resolved_id];
                        chunks.push(start);
                        i
                    }
                };
                self.saw_tool = true;
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
                                id: None,
                                r#type: None,
                                function: Some(FunctionCallChunk {
                                    name: None,
                                    arguments: Some(d.delta),
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
            ResponseStreamEvent::ResponseDone { response, .. }
            | ResponseStreamEvent::Completed { response, .. } => {
                self.model = response.model.clone();
                let usage = response.usage.as_ref().map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.total_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                });
                let finish_reason = if response.status == "incomplete" {
                    FinishReason::Length
                } else if self.saw_tool {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                };
                chunks.push(ChatCompletionChunk {
                    id: response.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChatChunkChoice {
                        index: 0,
                        delta: ChatChunkDelta::default(),
                        finish_reason: Some(finish_reason),
                        logprobs: None,
                    }],
                    usage,
                    system_fingerprint: None,
                    service_tier: None,
                });
                self.done = true;
            }
            ResponseStreamEvent::Incomplete { response, .. } => {
                self.model = response.model.clone();
                let usage = response.usage.as_ref().map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.total_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                });
                chunks.push(ChatCompletionChunk {
                    id: response.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChatChunkChoice {
                        index: 0,
                        delta: ChatChunkDelta::default(),
                        finish_reason: Some(FinishReason::Length),
                        logprobs: None,
                    }],
                    usage,
                    system_fingerprint: None,
                    service_tier: None,
                });
                self.done = true;
            }
            ResponseStreamEvent::Failed { response, .. } => {
                self.done = true;
                return Err(failed_conversion_error(&response));
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
                finish_reason: Some(if self.saw_tool {
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

/// FSM translating upstream OpenAI Chat chunks into Responses SSE events.
///
/// Emits the full official event sequence (R-SS-01..R-SS-15, R-SS-18, R-SS-19, R-SS-23) with a
/// strictly increasing `sequence_number`, and absorbs the `stream_options.include_usage` trailing
/// usage-only chunk before deciding the terminal usage (contract §8.4).
#[derive(Debug, Clone)]
pub struct ChatToResponsesFsm {
    emitter: ResponsesEmitter,
    tool_items: HashMap<u32, (String, String)>,
    tool_output_index: HashMap<u32, u32>,
    tool_names: HashMap<u32, String>,
    tool_args: HashMap<u32, String>,
    /// Finish reason already seen; the terminal event then waits for usage (or EOF) so a trailing
    /// usage-only chunk can still supply the real counts.
    pending_finish: Option<FinishReason>,
    pending_usage: Option<Usage>,
    done: bool,
}

impl ChatToResponsesFsm {
    pub fn new(fallback_model: &str) -> Self {
        let rid = format!("resp_{}", unique_suffix());
        Self {
            emitter: ResponsesEmitter::new(rid, fallback_model, now_secs() as i64),
            tool_items: HashMap::new(),
            tool_output_index: HashMap::new(),
            tool_names: HashMap::new(),
            tool_args: HashMap::new(),
            pending_finish: None,
            pending_usage: None,
            done: false,
        }
    }

    fn tool_output_items(&self) -> Vec<(u32, ResponseOutputItem)> {
        let mut indexes: Vec<u32> = self.tool_items.keys().cloned().collect();
        indexes.sort();
        let mut out = Vec::new();
        for idx in indexes {
            let (item_id, call_id) = self.tool_items[&idx].clone();
            let args = self.tool_args.get(&idx).cloned().unwrap_or_default();
            let name = self.tool_names.get(&idx).cloned().unwrap_or_default();
            let output_index = self.tool_output_index.get(&idx).cloned().unwrap_or(idx);
            out.push((
                output_index,
                ResponseOutputItem::FunctionCall {
                    id: item_id,
                    status: "completed".to_string(),
                    call_id,
                    name,
                    arguments: args,
                },
            ));
        }
        out
    }

    fn emit_terminal(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        let finish = self.pending_finish.clone().unwrap_or(FinishReason::Stop);
        let usage = self.pending_usage.clone();
        let tools = self.tool_output_items();
        events.push(self.emitter.terminal_event(&finish, usage.as_ref(), tools));
        self.done = true;
    }

    pub fn process_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
    ) -> Result<Vec<ResponseStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }
        self.emitter.model = chunk.model.clone();
        if chunk.created > 0 {
            self.emitter.created_at = chunk.created as i64;
        }

        // `stream_options.include_usage`: a usage-only chunk (`choices: []`) arrives after the
        // finish chunk. Absorb it for the terminal usage instead of treating it as content.
        if self.pending_finish.is_some() {
            if let Some(usage) = chunk.usage.clone() {
                self.pending_usage = Some(usage);
            }
            self.emit_terminal(&mut events);
            return Ok(events);
        }

        self.emitter.ensure_started(&mut events);

        for choice in &chunk.choices {
            // R-SS-12: the terminal event must be emitted exactly once and be the last frame, so a
            // multi-choice chunk cannot emit anything after a finish_reason already closed the
            // response.
            if self.done {
                break;
            }
            if let Some(ref reasoning) = choice.delta.reasoning_content {
                self.emitter.reasoning_delta(reasoning, &mut events);
            }
            if let Some(ref content) = choice.delta.content {
                self.emitter.text_delta(content, &mut events);
            }
            if let Some(ref tool_calls) = choice.delta.tool_calls {
                for tc in tool_calls {
                    let entry = self.tool_items.entry(tc.index).or_insert_with(|| {
                        let call_id = tc
                            .id
                            .clone()
                            .unwrap_or_else(|| format!("call_{}", tc.index));
                        (format!("fcitem_{}", tc.index), call_id)
                    });
                    let (item_id, call_id) = entry.clone();
                    if tc.id.is_some()
                        || tc.function.as_ref().and_then(|f| f.name.clone()).is_some()
                    {
                        let name = tc
                            .function
                            .as_ref()
                            .and_then(|f| f.name.clone())
                            .unwrap_or_default();
                        self.tool_names.insert(tc.index, name.clone());
                        let output_index = match self.tool_output_index.get(&tc.index) {
                            Some(i) => *i,
                            None => {
                                let i = self.emitter.allocate_index();
                                self.tool_output_index.insert(tc.index, i);
                                i
                            }
                        };
                        let seq = self.emitter.next_seq();
                        events.push(ResponseStreamEvent::OutputItemAdded {
                            sequence_number: seq,
                            response_id: String::new(),
                            output_index,
                            item: ResponseOutputItem::FunctionCall {
                                id: item_id,
                                status: "in_progress".to_string(),
                                call_id,
                                name,
                                arguments: String::new(),
                            },
                        });
                    }
                    if let Some(ref func) = tc.function {
                        if let Some(ref args) = func.arguments {
                            if !args.is_empty() {
                                let acc = self.tool_args.entry(tc.index).or_default();
                                acc.push_str(args);
                                let (tool_item_id, call_id) = self.tool_items[&tc.index].clone();
                                let output_index = self
                                    .tool_output_index
                                    .get(&tc.index)
                                    .cloned()
                                    .unwrap_or(tc.index);
                                let seq = self.emitter.next_seq();
                                events.push(ResponseStreamEvent::FunctionCallArgumentsDelta(
                                    ResponseFunctionCallDelta {
                                        sequence_number: seq,
                                        response_id: String::new(),
                                        item_id: tool_item_id,
                                        output_index,
                                        call_id,
                                        delta: args.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }
            }
            if let Some(ref finish) = choice.finish_reason {
                // Close the open item(s) now, then decide the terminal event: emit it immediately
                // when usage is already known, otherwise defer one chunk so the trailing
                // usage-only chunk is absorbed (contract §8.4).
                self.emitter.close_reasoning(&mut events);
                self.emitter.close_message(&mut events);
                self.pending_finish = Some(finish.clone());
                self.pending_usage = chunk.usage.clone();
                if self.pending_usage.is_some() {
                    self.emit_terminal(&mut events);
                }
            }
        }
        Ok(events)
    }

    /// Returns only the terminal event, preserving the historical signature. Callers that must
    /// close open items as well use [`Self::finish_open_events`].
    pub fn finish_if_open(&mut self) -> Option<ResponseStreamEvent> {
        self.finish_open_events().pop()
    }

    /// Full closing set for the EOF/abort path (R-SS-22): item/part `done` events followed by
    /// exactly one documented terminal event.
    pub fn finish_open_events(&mut self) -> Vec<ResponseStreamEvent> {
        if self.done {
            return Vec::new();
        }
        let finish = self.pending_finish.clone().unwrap_or(FinishReason::Stop);
        let usage = self.pending_usage.clone();
        let tools = self.tool_output_items();
        self.done = true;
        self.emitter.finish_open(&finish, usage.as_ref(), tools)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RespAnthropicBlock {
    Text,
    ToolUse { block_index: u32 },
}

/// FSM translating upstream Responses SSE into Anthropic Messages SSE.
/// Blocks follow done-driven lifecycle: a block closes only on its item/part
/// done event or at stream terminal, never when another block opens, so
/// interleaved tool declarations can never strand a delta on a closed block.
/// Text blocks open lazily on the first text delta. Thinking has no streaming
/// carrier on the Responses wire and is skipped.
#[derive(Debug, Clone)]
pub struct ResponsesToAnthropicFsm {
    message_id: String,
    model: String,
    block_counter: u32,
    active_block: Option<RespAnthropicBlock>,
    open_blocks: Vec<u32>,
    tool_blocks: HashMap<String, u32>,
    tool_item_to_id: HashMap<String, String>,
    sent_start: bool,
    saw_tool: bool,
    done: bool,
}

impl ResponsesToAnthropicFsm {
    pub fn new(fallback_model: &str) -> Self {
        Self {
            message_id: format!("msg_{}", uuid_simple()),
            model: fallback_model.to_string(),
            block_counter: 0,
            active_block: None,
            open_blocks: Vec::new(),
            tool_blocks: HashMap::new(),
            tool_item_to_id: HashMap::new(),
            sent_start: false,
            saw_tool: false,
            done: false,
        }
    }

    fn ensure_started(&mut self, events: &mut Vec<MessageStreamEvent>) {
        if !self.sent_start {
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
    }

    fn open_text_block(&mut self, events: &mut Vec<MessageStreamEvent>) -> u32 {
        self.ensure_started(events);
        if self.active_block == Some(RespAnthropicBlock::Text) {
            if let Some(idx) = self.open_blocks.last().cloned() {
                return idx;
            }
        }
        let idx = self.block_counter;
        self.block_counter += 1;
        self.open_blocks.push(idx);
        events.push(MessageStreamEvent::ContentBlockStart {
            index: idx,
            content_block: AnthropicContentBlock::Text {
                text: String::new(),
                cache_control: None,
            },
        });
        self.active_block = Some(RespAnthropicBlock::Text);
        idx
    }

    fn open_tool_block(
        &mut self,
        events: &mut Vec<MessageStreamEvent>,
        call_id: &str,
        name: &str,
    ) -> u32 {
        self.ensure_started(events);
        if let Some(&idx) = self.tool_blocks.get(call_id) {
            if self.open_blocks.contains(&idx) {
                self.active_block = Some(RespAnthropicBlock::ToolUse { block_index: idx });
                return idx;
            }
        }
        let idx = self.block_counter;
        self.block_counter += 1;
        self.open_blocks.push(idx);
        self.tool_blocks.insert(call_id.to_string(), idx);
        events.push(MessageStreamEvent::ContentBlockStart {
            index: idx,
            content_block: AnthropicContentBlock::ToolUse {
                id: call_id.to_string(),
                name: name.to_string(),
                input: serde_json::json!({}),
                cache_control: None,
            },
        });
        self.active_block = Some(RespAnthropicBlock::ToolUse { block_index: idx });
        self.saw_tool = true;
        idx
    }

    fn stop_block(&mut self, events: &mut Vec<MessageStreamEvent>, idx: u32) {
        if let Some(pos) = self.open_blocks.iter().position(|&i| i == idx) {
            self.open_blocks.remove(pos);
            events.push(MessageStreamEvent::ContentBlockStop { index: idx });
            if self.active_block == Some(RespAnthropicBlock::Text)
                || matches!(self.active_block, Some(RespAnthropicBlock::ToolUse { block_index }) if block_index == idx)
            {
                self.active_block = None;
            }
        }
    }

    fn stop_open_text_blocks(&mut self, events: &mut Vec<MessageStreamEvent>) {
        let tool_idx: std::collections::HashSet<u32> = self.tool_blocks.values().cloned().collect();
        let text_open: Vec<u32> = self
            .open_blocks
            .iter()
            .cloned()
            .filter(|i| !tool_idx.contains(i))
            .collect();
        for idx in text_open {
            self.stop_block(events, idx);
        }
    }

    fn close_and_finish(
        &mut self,
        events: &mut Vec<MessageStreamEvent>,
        stop: AnthropicStopReason,
        output_tokens: u32,
    ) {
        self.ensure_started(events);
        let remaining: Vec<u32> = std::mem::take(&mut self.open_blocks);
        for idx in remaining {
            events.push(MessageStreamEvent::ContentBlockStop { index: idx });
        }
        self.active_block = None;
        events.push(MessageStreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(stop),
                stop_sequence: None,
            },
            usage: Some(AnthropicDeltaUsage {
                output_tokens,
                ..Default::default()
            }),
        });
        events.push(MessageStreamEvent::MessageStop);
        self.done = true;
    }

    pub fn process_event(&mut self, event: ResponseStreamEvent) -> Result<Vec<MessageStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }
        match event {
            ResponseStreamEvent::ResponseCreated { response, .. } => {
                self.message_id = response.id.clone();
                self.model = response.model.clone();
                self.ensure_started(&mut events);
            }
            ResponseStreamEvent::TextDelta(d) => {
                if d.delta.is_empty() {
                    return Ok(events);
                }
                let idx = self.open_text_block(&mut events);
                events.push(MessageStreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: AnthropicDelta::TextDelta { text: d.delta },
                });
            }
            ResponseStreamEvent::OutputTextDelta(d) => {
                if d.delta.is_empty() {
                    return Ok(events);
                }
                let idx = self.open_text_block(&mut events);
                events.push(MessageStreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: AnthropicDelta::TextDelta { text: d.delta },
                });
            }
            ResponseStreamEvent::OutputItemAdded {
                item: ResponseOutputItem::FunctionCall { id, call_id, name, .. },
                ..
            } => {
                let effective_call_id = if !call_id.is_empty() {
                    call_id
                } else if !id.is_empty() {
                    id.clone()
                } else {
                    format!("call_{}", self.block_counter)
                };
                if !id.is_empty() {
                    self.tool_item_to_id.insert(id, effective_call_id.clone());
                }
                self.open_tool_block(&mut events, &effective_call_id, &name);
            }
            ResponseStreamEvent::OutputItemDone { item, .. } => match item {
                ResponseOutputItem::FunctionCall { id, call_id, .. } => {
                    let resolved_id = if !call_id.is_empty() {
                        call_id
                    } else if let Some(mapped) = self.tool_item_to_id.get(&id) {
                        mapped.clone()
                    } else {
                        id
                    };
                    if let Some(idx) = self.tool_blocks.get(&resolved_id).cloned() {
                        self.stop_block(&mut events, idx);
                    }
                }
                ResponseOutputItem::Message { .. } => {
                    self.stop_open_text_blocks(&mut events);
                }
                _ => {}
            },
            ResponseStreamEvent::ContentPartDone { .. } => {
                self.stop_open_text_blocks(&mut events);
            }
            ResponseStreamEvent::FunctionCallArgumentsDelta(d) => {
                let resolved_id = if !d.call_id.is_empty() {
                    d.call_id.clone()
                } else if let Some(mapped) = self.tool_item_to_id.get(&d.item_id) {
                    mapped.clone()
                } else if !d.item_id.is_empty() {
                    d.item_id.clone()
                } else {
                    String::new()
                };
                let idx = self.open_tool_block(&mut events, &resolved_id, "");
                events.push(MessageStreamEvent::ContentBlockDelta {
                    index: idx,
                    delta: AnthropicDelta::InputJsonDelta {
                        partial_json: d.delta,
                    },
                });
            }
            ResponseStreamEvent::ResponseDone { response, .. }
            | ResponseStreamEvent::Completed { response, .. }
            | ResponseStreamEvent::Incomplete { response, .. } => {
                let output_tokens = response
                    .usage
                    .as_ref()
                    .map(|u| u.output_tokens)
                    .unwrap_or(0);
                let stop = if self.saw_tool {
                    AnthropicStopReason::ToolUse
                } else if response.status == "incomplete" {
                    AnthropicStopReason::MaxTokens
                } else {
                    AnthropicStopReason::EndTurn
                };
                self.close_and_finish(&mut events, stop, output_tokens);
            }
            ResponseStreamEvent::Failed { response, .. } => {
                self.ensure_started(&mut events);
                events.push(MessageStreamEvent::Error {
                    error: AnthropicErrorDetail {
                        r#type: "api_error".to_string(),
                        message: upstream_failure_message(&response),
                    },
                });
                self.done = true;
            }
            _ => {}
        }
        Ok(events)
    }

    pub fn finish_if_open(&mut self) -> Option<Vec<MessageStreamEvent>> {
        if self.done {
            return None;
        }
        let mut events = Vec::new();
        let stop = if self.saw_tool {
            AnthropicStopReason::ToolUse
        } else {
            AnthropicStopReason::EndTurn
        };
        self.close_and_finish(&mut events, stop, 0);
        Some(events)
    }
}

/// Anthropic projection of a Failed response keeps the Ok+Error event
/// shape but passes the real upstream message through. Missing/empty
/// message falls back to the status (with code when present).
fn upstream_failure_message(response: &ResponseObject) -> String {
    if let Some(err) = response.error.as_ref() {
        if !err.message.trim().is_empty() {
            return err.message.clone();
        }
        if !err.code.trim().is_empty() {
            return format!("upstream response failed: status={} code={}", response.status, err.code);
        }
    }
    format!("upstream response failed: status={}", response.status)
}

/// FSM translating upstream Anthropic SSE into Responses SSE events.
///
/// Shares the [`ResponsesEmitter`] lifecycle with the chat-native path, so both produce the same
/// conformant wire sequence (R-SS-21 parity). Anthropic thinking blocks have no Responses streaming
/// carrier and are skipped.
#[derive(Debug, Clone)]
pub struct AnthropicToResponsesFsm {
    emitter: ResponsesEmitter,
    tool_items: HashMap<u32, (String, String)>,
    tool_output_index: HashMap<u32, u32>,
    tool_names: HashMap<u32, String>,
    tool_args: HashMap<u32, String>,
    done: bool,
}

impl AnthropicToResponsesFsm {
    pub fn new(fallback_model: &str) -> Self {
        let rid = format!("resp_{}", unique_suffix());
        Self {
            emitter: ResponsesEmitter::new(rid, fallback_model, now_secs() as i64),
            tool_items: HashMap::new(),
            tool_output_index: HashMap::new(),
            tool_names: HashMap::new(),
            tool_args: HashMap::new(),
            done: false,
        }
    }

    fn allocate_tool_index(&mut self, index: u32) -> u32 {
        match self.tool_output_index.get(&index) {
            Some(i) => *i,
            None => {
                let i = self.emitter.allocate_index();
                self.tool_output_index.insert(index, i);
                i
            }
        }
    }

    fn tool_output_items(&self) -> Vec<(u32, ResponseOutputItem)> {
        let mut indexes: Vec<u32> = self.tool_items.keys().cloned().collect();
        indexes.sort();
        let mut out = Vec::new();
        for idx in indexes {
            let (item_id, call_id) = self.tool_items[&idx].clone();
            let args = self
                .tool_args
                .get(&idx)
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            let name = self.tool_names.get(&idx).cloned().unwrap_or_default();
            let output_index = self.tool_output_index.get(&idx).cloned().unwrap_or(idx);
            out.push((
                output_index,
                ResponseOutputItem::FunctionCall {
                    id: item_id,
                    status: "completed".to_string(),
                    call_id,
                    name,
                    arguments: args,
                },
            ));
        }
        out
    }

    fn emit_terminal(
        &mut self,
        events: &mut Vec<ResponseStreamEvent>,
        finish: FinishReason,
        usage: Option<&Usage>,
    ) {
        let tools = self.tool_output_items();
        events.push(self.emitter.terminal_event(&finish, usage, tools));
        self.done = true;
    }

    pub fn process_event(&mut self, event: MessageStreamEvent) -> Result<Vec<ResponseStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }
        match event {
            MessageStreamEvent::MessageStart { message } => {
                self.emitter = ResponsesEmitter::new(
                    normalize_response_id(&message.id),
                    &message.model,
                    self.emitter.created_at,
                );
                self.emitter.ensure_started(&mut events);
            }
            MessageStreamEvent::ContentBlockStart {
                index,
                content_block: AnthropicContentBlock::ToolUse { id, name, .. },
            } => {
                self.emitter.ensure_started(&mut events);
                self.tool_items
                    .entry(index)
                    .or_insert((format!("fcitem_{}", index), id.clone()));
                self.tool_names.entry(index).or_insert(name.clone());
                let (item_id, call_id) = self.tool_items[&index].clone();
                let output_index = self.allocate_tool_index(index);
                let seq = self.emitter.next_seq();
                events.push(ResponseStreamEvent::OutputItemAdded {
                    sequence_number: seq,
                    response_id: String::new(),
                    output_index,
                    item: ResponseOutputItem::FunctionCall {
                        id: item_id,
                        status: "in_progress".to_string(),
                        call_id,
                        name,
                        arguments: String::new(),
                    },
                });
            }
            MessageStreamEvent::ContentBlockDelta { index, delta } => match delta {
                AnthropicDelta::TextDelta { text } => {
                    self.emitter.text_delta(&text, &mut events);
                }
                AnthropicDelta::InputJsonDelta { partial_json } => {
                    self.emitter.ensure_started(&mut events);
                    if !self.tool_items.contains_key(&index) {
                        self.tool_items
                            .entry(index)
                            .or_insert((format!("fcitem_{}", index), format!("call_{}", index)));
                    }
                    let acc = self.tool_args.entry(index).or_default();
                    acc.push_str(&partial_json);
                    let (tool_item_id, call_id) = self.tool_items[&index].clone();
                    let output_index = self.allocate_tool_index(index);
                    let seq = self.emitter.next_seq();
                    events.push(ResponseStreamEvent::FunctionCallArgumentsDelta(
                        ResponseFunctionCallDelta {
                            sequence_number: seq,
                            response_id: String::new(),
                            item_id: tool_item_id,
                            output_index,
                            call_id,
                            delta: partial_json,
                        },
                    ));
                }
                _ => {}
            },
            MessageStreamEvent::MessageDelta { delta, usage } => {
                let finish = match delta.stop_reason {
                    Some(AnthropicStopReason::MaxTokens) => FinishReason::Length,
                    _ => FinishReason::Stop,
                };
                let usage = usage.map(|u| {
                    let input_tokens = u.input_tokens.unwrap_or(0);
                    Usage {
                        prompt_tokens: input_tokens,
                        completion_tokens: u.output_tokens,
                        total_tokens: input_tokens + u.output_tokens,
                        prompt_tokens_details: None,
                        completion_tokens_details: None,
                    }
                });
                self.emit_terminal(&mut events, finish, usage.as_ref());
            }
            MessageStreamEvent::MessageStop if !self.done => {
                self.emit_terminal(&mut events, FinishReason::Stop, None);
            }
            _ => {}
        }
        Ok(events)
    }

    /// Returns only the terminal event, preserving the historical signature.
    pub fn finish_if_open(&mut self) -> Option<ResponseStreamEvent> {
        self.finish_open_events().pop()
    }

    /// Full closing set for the EOF/abort path (R-SS-21/R-SS-22).
    pub fn finish_open_events(&mut self) -> Vec<ResponseStreamEvent> {
        if self.done {
            return Vec::new();
        }
        let tools = self.tool_output_items();
        self.done = true;
        self.emitter.finish_open(&FinishReason::Stop, None, tools)
    }
}
