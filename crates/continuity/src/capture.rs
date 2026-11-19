use crate::types::{CaptureData, CaptureFormat, Event, SessionCase};
use serde_json::Value;
use std::collections::BTreeSet;

fn text(content: &Value) -> String {
    if let Some(value) = content.as_str() {
        return value.into();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn add(
    events: &mut Vec<Event>,
    ids: &mut BTreeSet<String>,
    id: String,
    role: String,
    body: String,
) -> Result<(), String> {
    if id.is_empty()
        || !ids.insert(id.clone())
        || !matches!(role.as_str(), "user" | "assistant" | "tool" | "system")
    {
        return Err("Duplicate/empty event ID or unsupported role".into());
    }
    if !body.trim().is_empty() {
        events.push(Event {
            id,
            role,
            text: body,
        });
    }
    Ok(())
}

pub fn parse(case: &SessionCase, bytes: &[u8]) -> Result<CaptureData, String> {
    let mut events = Vec::new();
    let mut ids = BTreeSet::new();
    let mut context_input_tokens = None;
    let mut context_window_tokens = case.policy.context_window_tokens;
    let mut gaps = case.capture.gaps.clone();
    if !case.capture.complete {
        gaps.push(
            "Capture completeness is not declared; unattended transfer is unavailable".into(),
        );
    }
    if matches!(case.capture.format, CaptureFormat::HermesExport) {
        let export: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Hermes export: {e}"))?;
        if export["session_id"].as_str() != Some(&case.session_id) {
            return Err("Hermes export belongs to a different session".into());
        }
        for (i, message) in export["messages"]
            .as_array()
            .ok_or("Hermes export requires messages")?
            .iter()
            .enumerate()
        {
            add(
                &mut events,
                &mut ids,
                format!("message:{i}"),
                message["role"].as_str().unwrap_or("").into(),
                text(&message["content"]),
            )?;
        }
    } else {
        let raw = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let mut session_seen = false;
        for (i, line) in raw.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(line).map_err(|e| format!("Capture line {}: {e}", i + 1))?;
            match case.capture.format {
                CaptureFormat::NormalizedJsonl => {
                    if value["session_id"].as_str() != Some(&case.session_id) {
                        return Err("Normalized event session mismatch".into());
                    }
                    session_seen = true;
                    add(
                        &mut events,
                        &mut ids,
                        value["id"].as_str().ok_or("Event requires id")?.into(),
                        value["role"].as_str().unwrap_or("").into(),
                        text(&value["text"]),
                    )?;
                }
                CaptureFormat::CodexJsonl => {
                    let payload = &value["payload"];
                    if value["type"] == "session_meta" {
                        if payload["id"].as_str() != Some(&case.session_id) {
                            return Err("Codex session mismatch".into());
                        }
                        session_seen = true;
                    } else if value["type"] == "response_item" {
                        match payload["type"].as_str() {
                            Some("message") => {
                                if payload["content"].as_array().is_some_and(|blocks| {
                                    blocks.iter().any(|b| {
                                        !matches!(
                                            b["type"].as_str(),
                                            Some("input_text" | "output_text" | "text")
                                        )
                                    })
                                }) {
                                    gaps.push(format!(
                                        "Nontext/unsupported Codex content at line {}",
                                        i + 1
                                    ));
                                }
                                add(
                                    &mut events,
                                    &mut ids,
                                    format!("line:{}", i + 1),
                                    payload["role"].as_str().unwrap_or("").into(),
                                    text(&payload["content"]),
                                )?;
                            }
                            Some("function_call_output" | "custom_tool_call_output") => add(
                                &mut events,
                                &mut ids,
                                format!("line:{}", i + 1),
                                "tool".into(),
                                text(&payload["output"]),
                            )?,
                            Some(
                                "function_call" | "custom_tool_call" | "reasoning" | "compaction",
                            ) => {}
                            _ => gaps.push(format!(
                                "Unsupported Codex response record at line {}",
                                i + 1
                            )),
                        }
                    } else if value["type"] == "event_msg" && payload["type"] == "token_count" {
                        context_input_tokens =
                            payload["info"]["last_token_usage"]["input_tokens"].as_u64();
                        context_window_tokens = payload["info"]["model_context_window"]
                            .as_u64()
                            .or(context_window_tokens);
                    }
                }
                CaptureFormat::ClaudeJsonl => {
                    if let Some(session) = value["sessionId"].as_str() {
                        if session != case.session_id {
                            return Err("Claude session mismatch".into());
                        }
                        session_seen = true;
                    }
                    let message = &value["message"];
                    if matches!(value["type"].as_str(), Some("user" | "assistant")) {
                        let content = &message["content"];
                        let body = text(content);
                        if !body.is_empty() {
                            add(
                                &mut events,
                                &mut ids,
                                format!("line:{}", i + 1),
                                message["role"].as_str().unwrap_or("").into(),
                                body,
                            )?;
                        }
                        if let Some(blocks) = content.as_array() {
                            for (j, block) in blocks.iter().enumerate() {
                                if block["type"] == "tool_result" {
                                    add(
                                        &mut events,
                                        &mut ids,
                                        format!("line:{}:tool:{j}", i + 1),
                                        "tool".into(),
                                        text(&block["content"]),
                                    )?;
                                } else if !matches!(
                                    block["type"].as_str(),
                                    Some("text" | "tool_use" | "thinking" | "redacted_thinking")
                                ) {
                                    gaps.push(format!(
                                        "Nontext/unsupported Claude content at line {}",
                                        i + 1
                                    ));
                                }
                            }
                        }
                        if let Some(input) = message["usage"]["input_tokens"].as_u64() {
                            context_input_tokens = Some(
                                input
                                    .saturating_add(
                                        message["usage"]["cache_read_input_tokens"]
                                            .as_u64()
                                            .unwrap_or(0),
                                    )
                                    .saturating_add(
                                        message["usage"]["cache_creation_input_tokens"]
                                            .as_u64()
                                            .unwrap_or(0),
                                    ),
                            );
                        }
                    }
                }
                CaptureFormat::HermesExport => unreachable!(),
            }
            if events.len() > 20_000 {
                return Err("Capture exceeds 20,000 message events".into());
            }
        }
        if !session_seen {
            return Err("Capture contains no matching session identity".into());
        }
    }
    if events.iter().all(|event| event.role != "user") {
        return Err("Capture contains no user messages; coverage cannot pass vacuously".into());
    }
    Ok(CaptureData {
        events,
        bytes: bytes.len() as u64,
        context_input_tokens,
        context_window_tokens,
        gaps,
    })
}
