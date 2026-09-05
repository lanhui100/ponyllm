use crate::anthropic::messages::*;
use crate::error::Result;
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

fn skeleton_response(id: &str, model: &str) -> ResponseObject {
    ResponseObject {
        id: id.to_string(),
        object: "response".to_string(),
        status: "in_progress".to_string(),
        model: model.to_string(),
        output: vec![],
        usage: None,
        error: None,
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
            ResponseStreamEvent::ResponseCreated { response } => {
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
            ResponseStreamEvent::TextDelta(d) | ResponseStreamEvent::OutputTextDelta(d) => {
                chunks.push(chat_text_chunk(
                    &self.response_id,
                    &self.model,
                    self.created,
                    d.delta,
                ));
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
            ResponseStreamEvent::ResponseDone { response }
            | ResponseStreamEvent::Completed { response } => {
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
                        finish_reason: Some(if self.saw_tool {
                            FinishReason::ToolCalls
                        } else {
                            FinishReason::Stop
                        }),
                        logprobs: None,
                    }],
                    usage,
                    system_fingerprint: None,
                    service_tier: None,
                });
                self.done = true;
            }
            ResponseStreamEvent::Failed { response } => {
                chunks.push(ChatCompletionChunk {
                    id: response.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChatChunkChoice {
                        index: 0,
                        delta: ChatChunkDelta::default(),
                        finish_reason: Some(FinishReason::Other),
                        logprobs: None,
                    }],
                    usage: None,
                    system_fingerprint: None,
                    service_tier: None,
                });
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
/// The terminal `response.completed` carries the accumulated text and tool calls.
#[derive(Debug, Clone)]
pub struct ChatToResponsesFsm {
    response_id: String,
    model: String,
    item_id: String,
    tool_items: HashMap<u32, (String, String)>,
    tool_names: HashMap<u32, String>,
    tool_args: HashMap<u32, String>,
    text_acc: String,
    sent_created: bool,
    done: bool,
}

impl ChatToResponsesFsm {
    pub fn new(fallback_model: &str) -> Self {
        let rid = format!("resp_{}", uuid_simple());
        Self {
            response_id: rid.clone(),
            model: fallback_model.to_string(),
            item_id: format!("{}_item0", rid),
            tool_items: HashMap::new(),
            tool_names: HashMap::new(),
            tool_args: HashMap::new(),
            text_acc: String::new(),
            sent_created: false,
            done: false,
        }
    }

    fn ensure_created(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if !self.sent_created {
            self.sent_created = true;
            events.push(ResponseStreamEvent::ResponseCreated {
                response: skeleton_response(&self.response_id, &self.model),
            });
        }
    }

    fn completed_event(&self, finish: &FinishReason, usage: Option<Usage>) -> ResponseStreamEvent {
        let mut output = Vec::new();
        if !self.text_acc.is_empty() {
            output.push(ResponseOutputItem::Message {
                id: self.item_id.clone(),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: vec![ResponseContentPart::Text {
                    text: self.text_acc.clone(),
                }],
            });
        }
        let mut indexes: Vec<u32> = self.tool_items.keys().cloned().collect();
        indexes.sort();
        for idx in indexes {
            let (item_id, call_id) = self.tool_items[&idx].clone();
            let args = self.tool_args.get(&idx).cloned().unwrap_or_default();
            let name = self.tool_names.get(&idx).cloned().unwrap_or_default();
            output.push(ResponseOutputItem::FunctionCall {
                id: item_id,
                status: "completed".to_string(),
                call_id,
                name,
                arguments: args,
            });
        }
        let status = if matches!(finish, FinishReason::Length) {
            "incomplete"
        } else {
            "completed"
        };
        let (input_tokens, output_tokens) = usage
            .as_ref()
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .unwrap_or((0, 0));
        ResponseStreamEvent::Completed {
            response: ResponseObject {
                id: self.response_id.clone(),
                object: "response".to_string(),
                status: status.to_string(),
                model: self.model.clone(),
                output,
                usage: Some(ResponseUsage {
                    input_tokens,
                    output_tokens,
                    total_tokens: input_tokens + output_tokens,
                }),
                error: None,
            },
        }
    }

    pub fn process_chunk(
        &mut self,
        chunk: ChatCompletionChunk,
    ) -> Result<Vec<ResponseStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }
        self.model = chunk.model.clone();
        self.ensure_created(&mut events);

        for choice in &chunk.choices {
            if let Some(ref content) = choice.delta.content {
                if !content.is_empty() {
                    self.text_acc.push_str(content);
                    events.push(ResponseStreamEvent::OutputTextDelta(ResponseTextDelta {
                        response_id: self.response_id.clone(),
                        item_id: self.item_id.clone(),
                        output_index: 0,
                        content_index: 0,
                        delta: content.clone(),
                    }));
                }
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
                        events.push(ResponseStreamEvent::OutputItemAdded {
                            response_id: self.response_id.clone(),
                            output_index: tc.index,
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
                                events.push(ResponseStreamEvent::FunctionCallArgumentsDelta(
                                    ResponseFunctionCallDelta {
                                        response_id: self.response_id.clone(),
                                        item_id: tool_item_id,
                                        output_index: tc.index,
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
                self.done = true;
                events.push(self.completed_event(finish, chunk.usage.clone()));
            }
        }
        Ok(events)
    }

    pub fn finish_if_open(&mut self) -> Option<ResponseStreamEvent> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(self.completed_event(&FinishReason::Stop, None))
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
            usage: Some(AnthropicDeltaUsage { output_tokens }),
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
            ResponseStreamEvent::ResponseCreated { response } => {
                self.message_id = response.id.clone();
                self.model = response.model.clone();
                self.ensure_started(&mut events);
            }
            ResponseStreamEvent::TextDelta(d) | ResponseStreamEvent::OutputTextDelta(d) => {
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
            ResponseStreamEvent::ResponseDone { response }
            | ResponseStreamEvent::Completed { response } => {
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
            ResponseStreamEvent::Failed { .. } => {
                self.ensure_started(&mut events);
                events.push(MessageStreamEvent::Error {
                    error: AnthropicErrorDetail {
                        r#type: "api_error".to_string(),
                        message: "upstream response failed".to_string(),
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

/// FSM translating upstream Anthropic SSE into Responses SSE events.
/// Thinking blocks have no Responses streaming carrier and are skipped.
#[derive(Debug, Clone)]
pub struct AnthropicToResponsesFsm {
    response_id: String,
    model: String,
    item_id: String,
    text_acc: String,
    tool_items: HashMap<u32, (String, String)>,
    tool_names: HashMap<u32, String>,
    tool_args: HashMap<u32, String>,
    sent_created: bool,
    done: bool,
}

impl AnthropicToResponsesFsm {
    pub fn new(fallback_model: &str) -> Self {
        let rid = format!("resp_{}", uuid_simple());
        Self {
            response_id: rid.clone(),
            model: fallback_model.to_string(),
            item_id: format!("{}_item0", rid),
            text_acc: String::new(),
            tool_items: HashMap::new(),
            tool_names: HashMap::new(),
            tool_args: HashMap::new(),
            sent_created: false,
            done: false,
        }
    }

    fn ensure_created(&mut self, events: &mut Vec<ResponseStreamEvent>) {
        if !self.sent_created {
            self.sent_created = true;
            events.push(ResponseStreamEvent::ResponseCreated {
                response: skeleton_response(&self.response_id, &self.model),
            });
        }
    }

    fn completed_event(&self, status: &str, output_tokens: u32) -> ResponseStreamEvent {
        let mut output = Vec::new();
        if !self.text_acc.is_empty() {
            output.push(ResponseOutputItem::Message {
                id: self.item_id.clone(),
                status: "completed".to_string(),
                role: "assistant".to_string(),
                content: vec![ResponseContentPart::Text {
                    text: self.text_acc.clone(),
                }],
            });
        }
        let mut indexes: Vec<u32> = self.tool_items.keys().cloned().collect();
        indexes.sort();
        for idx in indexes {
            let (item_id, call_id) = self.tool_items[&idx].clone();
            let args = self
                .tool_args
                .get(&idx)
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            let name = self.tool_names.get(&idx).cloned().unwrap_or_default();
            output.push(ResponseOutputItem::FunctionCall {
                id: item_id,
                status: "completed".to_string(),
                call_id,
                name,
                arguments: args,
            });
        }
        ResponseStreamEvent::Completed {
            response: ResponseObject {
                id: self.response_id.clone(),
                object: "response".to_string(),
                status: status.to_string(),
                model: self.model.clone(),
                output,
                usage: Some(ResponseUsage {
                    input_tokens: 0,
                    output_tokens,
                    total_tokens: output_tokens,
                }),
                error: None,
            },
        }
    }

    pub fn process_event(&mut self, event: MessageStreamEvent) -> Result<Vec<ResponseStreamEvent>> {
        let mut events = Vec::new();
        if self.done {
            return Ok(events);
        }
        match event {
            MessageStreamEvent::MessageStart { message } => {
                self.response_id = message.id.clone();
                self.item_id = format!("{}_item0", self.response_id);
                self.model = message.model.clone();
                self.ensure_created(&mut events);
            }
            MessageStreamEvent::ContentBlockStart {
                index,
                content_block: AnthropicContentBlock::ToolUse { id, name, .. },
            } => {
                self.ensure_created(&mut events);
                self.tool_items
                    .entry(index)
                    .or_insert((format!("fcitem_{}", index), id.clone()));
                self.tool_names.entry(index).or_insert(name.clone());
                let (item_id, call_id) = self.tool_items[&index].clone();
                events.push(ResponseStreamEvent::OutputItemAdded {
                    response_id: self.response_id.clone(),
                    output_index: index,
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
                    self.ensure_created(&mut events);
                    self.text_acc.push_str(&text);
                    events.push(ResponseStreamEvent::OutputTextDelta(ResponseTextDelta {
                        response_id: self.response_id.clone(),
                        item_id: self.item_id.clone(),
                        output_index: 0,
                        content_index: 0,
                        delta: text,
                    }));
                }
                AnthropicDelta::InputJsonDelta { partial_json } => {
                    self.ensure_created(&mut events);
                    if !self.tool_items.contains_key(&index) {
                        self.tool_items
                            .entry(index)
                            .or_insert((format!("fcitem_{}", index), format!("call_{}", index)));
                    }
                    let acc = self.tool_args.entry(index).or_default();
                    acc.push_str(&partial_json);
                    let (tool_item_id, call_id) = self.tool_items[&index].clone();
                    events.push(ResponseStreamEvent::FunctionCallArgumentsDelta(
                        ResponseFunctionCallDelta {
                            response_id: self.response_id.clone(),
                            item_id: tool_item_id,
                            output_index: index,
                            call_id,
                            delta: partial_json,
                        },
                    ));
                }
                _ => {}
            },
            MessageStreamEvent::MessageDelta { delta, usage } => {
                let output_tokens = usage.map(|u| u.output_tokens).unwrap_or(0);
                let status = match delta.stop_reason {
                    Some(AnthropicStopReason::MaxTokens) => "incomplete",
                    _ => "completed",
                };
                self.done = true;
                self.ensure_created(&mut events);
                events.push(self.completed_event(status, output_tokens));
            }
            MessageStreamEvent::MessageStop if !self.done => {
                self.done = true;
                self.ensure_created(&mut events);
                events.push(self.completed_event("completed", 0));
            }
            _ => {}
        }
        Ok(events)
    }

    pub fn finish_if_open(&mut self) -> Option<ResponseStreamEvent> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(self.completed_event("completed", 0))
    }
}
