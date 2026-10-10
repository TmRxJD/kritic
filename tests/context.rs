//! kritic's lossless context compaction (the Hermes context engine backend).

use std::collections::BTreeMap;
use std::sync::Mutex;

use kritic::context::{
    Archive, CompressInput, RecallFormat, RecallOptions, archive_id, compress, recall,
    recall_with_options, text_of, total_tokens,
};
use kritic::kev::{Answer, Decider, Decision, Question};
use serde_json::{Value, json};

#[test]
fn session_paths_and_hashes_do_not_alias() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = ["a/b", "a_b", "A", "a", "", "default"];
    let paths: std::collections::HashSet<_> = sessions
        .iter()
        .map(|s| Archive::for_session(dir.path(), s).path)
        .collect();
    assert_eq!(paths.len(), sessions.len());
    let prefix = "x".repeat(96);
    assert_ne!(
        Archive::for_session(dir.path(), &format!("{prefix}a")).path,
        Archive::for_session(dir.path(), &format!("{prefix}b")).path
    );
    assert_eq!(archive_id("tool", "payload").len(), 67);
    let legacy = dir.path().join("context/CASE");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("archive.jsonl"), "malformed").unwrap();
    assert!(Archive::for_session(dir.path(), "CASE").load().is_err());
    assert!(
        Archive::for_session(dir.path(), "case")
            .load()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn exact_content_and_legacy_recall_survive() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "legacy");
    let parts =
        json!([{"type":"text","text":"first"},{"type":"text","text":"second","extra":true}]);
    let inp: CompressInput = serde_json::from_value(json!({"messages":[{"role":"tool","content":parts,"tool_call_id":"call"}],"target_tokens":1,"session_id":"legacy","protect_first_n":0,"protect_last_n":0,"big_chars":1,"use_kev":false})).unwrap();
    let out = compress(None::<&Relevance>, &archive, &inp).await;
    assert_eq!(out.archived.len(), 1);
    assert_ne!(out.archived[0].id, archive_id("tool", &parts.to_string()));
    let recalled = recall(&archive, Some(&out.archived[0].id), None, 1000);
    assert!(recalled.found[0].original_content.is_none());
    let exact = recall_with_options(
        &archive,
        Some(&out.archived[0].id),
        None,
        RecallOptions {
            format: RecallFormat::Original,
            max_chars: 1000,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(exact.found[0].original_content.as_ref(), Some(&parts));
    assert!(exact.found[0].content.is_empty());
    assert_eq!(out.messages[0]["tool_call_id"], "call");
    let old = dir.path().join("context/legacy");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("archive.jsonl"), r#"{"id":"kr-012345abcdef","role":"tool","content":"old record","tokens":2,"why":"old","at":"old"}"#).unwrap();
    assert_eq!(
        recall(&archive, Some("kr-012345abcdef"), None, 1000).found[0].content,
        "old record"
    );
}

#[tokio::test]
async fn corrupt_archive_preserves_input_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "corrupt");
    std::fs::create_dir_all(archive.path.parent().unwrap()).unwrap();
    std::fs::write(&archive.path, "{broken\n").unwrap();
    let msgs = transcript();
    let out = compress(None::<&Relevance>, &archive, &input(msgs.clone(), 1)).await;
    assert_eq!(out.messages, msgs);
    assert!(out.archived.is_empty());
    assert!(
        out.notes
            .iter()
            .any(|s| s.contains("invalid archive record at line 1"))
    );
    assert!(
        recall(&archive, Some("missing"), None, 100)
            .error
            .unwrap()
            .contains("invalid archive record")
    );
    std::fs::remove_file(&archive.path).unwrap();
    std::fs::create_dir(&archive.path).unwrap();
    assert!(archive.load().is_err());
}

#[tokio::test]
async fn recall_budgets_cover_typed_json_and_preserve_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "bounded");
    let parts = json!([{"type":"text", "text":"é🦀ok", "extra":"x".repeat(100_000)}]);
    let input: CompressInput = serde_json::from_value(json!({"messages":[{"role":"tool","content":parts}],"session_id":"bounded","target_tokens":1,"protect_first_n":0,"protect_last_n":0,"big_chars":1,"use_kev":false})).unwrap();
    let compressed = compress(None::<&Relevance>, &archive, &input).await;
    let id = &compressed.archived[0].id;
    let text = recall(&archive, Some(id), None, 2);
    assert_eq!(text.found[0].content, "é");
    assert!(text.found[0].original_content.is_none());
    assert!(serde_json::to_vec(&text).unwrap().len() < 1000);
    assert!(recall(&archive, Some(id), None, 1).found.is_empty());
    let options = RecallOptions {
        format: RecallFormat::Original,
        max_chars: 200_000,
        max_output_bytes: 200_000,
    };
    let exact = recall_with_options(&archive, Some(id), None, options).unwrap();
    assert_eq!(exact.found[0].original_content.as_ref(), Some(&parts));
    assert!(exact.found[0].content.is_empty());
    assert!(serde_json::to_vec(&exact).unwrap().len() <= options.max_output_bytes);
    for limited in [
        RecallOptions {
            max_chars: 2,
            ..options
        },
        RecallOptions {
            max_output_bytes: 256,
            ..options
        },
    ] {
        let result = recall_with_options(&archive, Some(id), None, limited).unwrap();
        assert!(result.found.is_empty());
        assert!(result.error.unwrap().contains("exceeds"));
    }
    assert_eq!(
        archive.load().unwrap()[0].original_content.as_ref(),
        Some(&parts)
    );
    assert!(
        recall_with_options(
            &archive,
            Some(id),
            None,
            RecallOptions {
                max_output_bytes: 0,
                ..options
            }
        )
        .is_err()
    );
}

#[test]
fn recall_wire_limit_includes_escaping_metadata_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "wire");
    std::fs::create_dir_all(archive.path.parent().unwrap()).unwrap();
    let mut record = json!({"id":"kr-wire","role":"tool","content":"\"\n🦀".repeat(1000),"original_content":"duplicate","tokens":1000,"why":"test","at":"now"});
    let options = RecallOptions {
        max_chars: 10_000,
        max_output_bytes: 256,
        ..Default::default()
    };
    std::fs::write(&archive.path, record.to_string()).unwrap();
    let recalled = recall_with_options(&archive, Some("kr-wire"), None, options).unwrap();
    assert!(serde_json::to_vec(&recalled).unwrap().len() <= 256);
    assert!(recalled.truncated_chars > 0);
    assert!(recalled.found.iter().all(|a| a.original_content.is_none()));
    record["message_uid"] = "large metadata".repeat(1000).into();
    std::fs::write(&archive.path, record.to_string()).unwrap();
    let recalled = recall_with_options(&archive, Some("kr-wire"), None, options).unwrap();
    assert!(recalled.found.is_empty());
    assert!(serde_json::to_vec(&recalled).unwrap().len() <= 256);
    let missing =
        recall_with_options(&archive, Some(&"missing".repeat(1000)), None, options).unwrap();
    assert!(missing.error.is_some());
    assert!(serde_json::to_vec(&missing).unwrap().len() <= 256);
}

#[test]
fn archive_process_writer() {
    let Ok(root) = std::env::var("KRITIC_CONTEXT_TEST_ROOT") else {
        return;
    };
    let writer = std::env::var("KRITIC_CONTEXT_TEST_WRITER").unwrap();
    let archive = Archive::for_session(std::path::Path::new(&root), "parallel");
    let rt = tokio::runtime::Runtime::new().unwrap();
    for _ in 0..5 {
        let inp: CompressInput = serde_json::from_value(json!({"messages":[{"role":"tool","content":format!("writer {writer} {}", "x".repeat(10000))}],"target_tokens":1,"session_id":"parallel","protect_first_n":0,"protect_last_n":0,"big_chars":1,"use_kev":false})).unwrap();
        let out = rt.block_on(compress(None::<&Relevance>, &archive, &inp));
        assert_eq!(out.archived.len(), 1, "{:?}", out.notes);
    }
}

#[test]
fn simultaneous_process_appends_are_complete_and_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let mut children: Vec<_> = (0..4)
        .map(|i| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "archive_process_writer"])
                .env("KRITIC_CONTEXT_TEST_ROOT", dir.path())
                .env("KRITIC_CONTEXT_TEST_WRITER", i.to_string())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }
    let records = Archive::for_session(dir.path(), "parallel").load().unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records
            .iter()
            .map(|a| &a.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );
}

/// Relevance = 0.9 when the chunk mentions "parser", else 0.1. Counts calls.
struct Relevance {
    calls: Mutex<usize>,
    fail: bool,
}

impl Decider for Relevance {
    async fn decide(
        &self,
        state: &Value,
        questions: &BTreeMap<String, Question>,
    ) -> Result<Decision, String> {
        *self.calls.lock().unwrap() += 1;
        if self.fail {
            return Err("laya unreachable".into());
        }
        let chunk = state["chunk"].as_str().unwrap_or("");
        let p = if chunk.contains("parser") { 0.9 } else { 0.1 };
        let mut d = Decision::default();
        for id in questions.keys() {
            d.answers.insert(
                id.clone(),
                Answer {
                    value: p,
                    choice: None,
                    probabilities: None,
                    confidence: 0.9,
                },
            );
        }
        Ok(d)
    }
}

fn rel() -> Relevance {
    Relevance {
        calls: Mutex::new(0),
        fail: false,
    }
}

/// A realistic agent transcript: system, the request, tool calls with results, replies.
fn transcript() -> Vec<Value> {
    let mut m = vec![
        json!({"role": "system", "content": "You are a coding agent."}),
        json!({"role": "user", "content": "Fix the parser bug in src/parse.rs", "message_uid": "u0"}),
    ];
    for i in 0..8 {
        let id = format!("call_{i}");
        let topic = if i % 3 == 0 {
            "parser tokenizer state machine"
        } else {
            "unrelated CSS color tokens"
        };
        m.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function", "function": {"name": "read_file", "arguments": format!("{{\"path\": \"f{i}.rs\"}}")}}]}));
        m.push(json!({"role": "tool", "tool_call_id": id, "content": format!("f{i}.rs: {topic} {}", "line of code\n".repeat(200)), "message_uid": format!("t{i}")}));
        m.push(json!({"role": "assistant", "content": format!("Looked at f{i}.rs: {topic}.")}));
    }
    m.push(json!({"role": "user", "content": "Now make the parser handle empty input."}));
    m
}

fn input(messages: Vec<Value>, target: usize) -> CompressInput {
    // Head: the request only; tail: the latest exchange. Everything between is fair game.
    serde_json::from_value(json!({"messages": messages, "target_tokens": target, "session_id": "s1", "protect_first_n": 1, "protect_last_n": 2})).unwrap()
}

#[tokio::test]
async fn compaction_is_lossless_structural_and_relevance_ordered() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "s1");
    let msgs = transcript();
    let before = total_tokens(&msgs);
    let target = before / 3;
    let d = rel();
    let out = compress(Some(&d), &archive, &input(msgs.clone(), target)).await;

    // Shrinks to the target, by archiving.
    assert!(
        out.tokens_after <= target,
        "{} > {target}",
        out.tokens_after
    );
    assert_eq!(out.ranking, "kev");
    assert!(!out.archived.is_empty());

    // Structure is untouched: same length, roles, tool_call ids, tool_calls, host uids.
    assert_eq!(out.messages.len(), msgs.len());
    for (a, b) in out.messages.iter().zip(&msgs) {
        assert_eq!(a["role"], b["role"]);
        assert_eq!(a.get("tool_call_id"), b.get("tool_call_id"));
        assert_eq!(
            a.get("tool_calls"),
            b.get("tool_calls"),
            "tool-call arguments are execution records"
        );
        assert_eq!(a.get("message_uid"), b.get("message_uid"));
    }
    // System, the first request and the protected tail are verbatim.
    assert_eq!(out.messages[0], msgs[0]);
    assert_eq!(out.messages[1], msgs[1]);
    assert_eq!(out.messages.last(), msgs.last());

    // Irrelevant results go before relevant ones: every archived parser result was archived only
    // after every CSS one.
    let archived_tools: Vec<String> = out
        .archived
        .iter()
        .filter(|a| msgs[a.index]["role"] == "tool")
        .map(|a| text_of(&msgs[a.index]))
        .collect();
    let css = archived_tools.iter().filter(|t| t.contains("CSS")).count();
    let total_css = msgs
        .iter()
        .filter(|m| m["role"] == "tool" && text_of(m).contains("CSS"))
        .count();
    assert!(css > 0, "irrelevant tool results are archived");
    if archived_tools
        .iter()
        .any(|t| t.contains("parser tokenizer"))
    {
        assert_eq!(
            css, total_css,
            "a relevant result was archived while an irrelevant one stayed"
        );
    }

    // Lossless: every stub's id recalls the original content exactly.
    for a in &out.archived {
        let stub = text_of(&out.messages[a.index]);
        assert!(
            stub.contains(&a.id) && stub.contains("kritic_recall"),
            "{stub}"
        );
        let r = recall(&archive, Some(&a.id), None, 1_000_000);
        assert_eq!(r.found.len(), 1);
        assert_eq!(r.found[0].content, text_of(&msgs[a.index]));
    }
    // Search across the archive.
    let r = recall(&archive, None, Some("css COLOR"), 1_000_000);
    assert!(
        !r.found.is_empty()
            && r.found
                .iter()
                .all(|a| a.content.to_lowercase().contains("css color"))
    );
    // Output limit.
    let r = recall(&archive, None, Some("line of code"), 100);
    assert!(r.found.iter().map(|a| a.content.len()).sum::<usize>() <= 100 && r.truncated_chars > 0);

    // Stable ids: compressing the same input again archives the same ids and writes no duplicates.
    let again = compress(Some(&rel()), &archive, &input(msgs.clone(), target)).await;
    assert_eq!(
        again.archived.iter().map(|a| &a.id).collect::<Vec<_>>(),
        out.archived.iter().map(|a| &a.id).collect::<Vec<_>>()
    );
    assert_eq!(archive.load().unwrap().len(), out.archived.len());
    // Already-compacted input is left alone: stubs are never re-archived.
    let third = compress(Some(&rel()), &archive, &input(out.messages.clone(), target)).await;
    assert!(third.archived.is_empty());
    assert_eq!(third.messages, out.messages);
}

#[tokio::test]
async fn prune_needs_no_model_and_duplicates_go_first() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "s2");
    let big = "x".repeat(9000);
    let msgs = vec![
        json!({"role": "system", "content": "sys"}),
        json!({"role": "user", "content": "task"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "a", "type": "function", "function": {"name": "ls", "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": "a", "content": "same listing"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "b", "type": "function", "function": {"name": "cat", "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": "b", "content": big.clone()}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c", "type": "function", "function": {"name": "ls", "arguments": "{}"}}]}),
        json!({"role": "tool", "tool_call_id": "c", "content": "same listing"}),
        json!({"role": "user", "content": "a"}),
        json!({"role": "user", "content": "b"}),
        json!({"role": "user", "content": "c"}),
        json!({"role": "user", "content": "d"}),
        json!({"role": "user", "content": "e"}),
        json!({"role": "user", "content": "f"}),
    ];
    let mut inp = input(msgs.clone(), 1_000_000);
    inp.mode = "prune".into();
    let d = rel();
    let out = compress(Some(&d), &archive, &inp).await;
    assert_eq!(*d.calls.lock().unwrap(), 0, "prune never asks the model");
    let whys: Vec<&str> = out.archived.iter().map(|a| a.why.as_str()).collect();
    assert_eq!(
        whys,
        ["duplicate of a later tool result", "large tool result"]
    );
    assert_eq!(
        out.archived[0].index, 3,
        "the earlier duplicate goes; the latest copy stays"
    );
    assert_eq!(out.messages[7], msgs[7]);
    assert_eq!(out.archived[1].id, archive_id("tool", &big));
}

#[tokio::test]
async fn without_kev_it_falls_back_to_oldest_first_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "s3");
    let msgs = transcript();
    let target = total_tokens(&msgs) / 2;
    let d = Relevance {
        calls: Mutex::new(0),
        fail: true,
    };
    let out = compress(Some(&d), &archive, &input(msgs.clone(), target)).await;
    assert_eq!(out.ranking, "recency");
    assert!(
        out.notes.iter().any(|n| n.contains("Kev unavailable")),
        "{:?}",
        out.notes
    );
    assert!(out.tokens_after <= target);
    let first = out.archived.iter().map(|a| a.index).min().unwrap();
    // Index 2 is the first tool call: no text to archive (its arguments are never touched).
    assert_eq!(
        first, 3,
        "the oldest unprotected message with content goes first"
    );

    // No decider at all: same order, no notes about Kev failing.
    let out2 = compress(
        None::<&Relevance>,
        &Archive::for_session(dir.path(), "s4"),
        &input(msgs, target),
    )
    .await;
    assert_eq!(out2.ranking, "recency");
    assert_eq!(
        out2.archived.iter().map(|a| a.index).collect::<Vec<_>>(),
        out.archived.iter().map(|a| a.index).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn an_unwritable_archive_archives_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the session directory should be: the archive can't be created.
    std::fs::write(dir.path().join("context"), "not a dir").unwrap();
    let archive = Archive::for_session(dir.path(), "s5");
    let msgs = transcript();
    let out = compress(Some(&rel()), &archive, &input(msgs.clone(), 10)).await;
    assert_eq!(
        out.messages, msgs,
        "a stub must never point at an archive that doesn't exist"
    );
    assert!(
        out.archived.is_empty() && out.notes.iter().any(|n| n.contains("nothing was archived"))
    );
}

#[test]
fn multimodal_and_protected_messages_are_never_archived() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let archive = Archive::for_session(dir.path(), "s6");
    let image = json!({"role": "user", "content": [{"type": "text", "text": "see this"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]});
    let mut msgs = vec![
        json!({"role": "system", "content": "sys"}),
        json!({"role": "user", "content": "go"}),
    ];
    msgs.push(image.clone());
    for i in 0..10 {
        msgs.push(
            json!({"role": "assistant", "content": format!("step {i} {}", "words ".repeat(300))}),
        );
    }
    let mut inp = input(msgs.clone(), 1);
    inp.protect_first_n = 1;
    inp.protect_last_n = 2;
    let out = rt.block_on(compress(None::<&Relevance>, &archive, &inp));
    assert_eq!(out.messages[2], image);
    assert!(
        out.archived
            .iter()
            .all(|a| a.index >= 3 && a.index < msgs.len() - 2)
    );
    assert!(out.notes.iter().any(|n| n.contains("over the target")));
}

#[test]
fn hermes_usage_summarises_the_latest_session() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("hermes-usage.jsonl");
    let mut lines =
        vec![r#"{"session_id":"old","prompt_tokens":5,"context_length":100}"#.to_string()];
    for (i, p) in [1000u64, 3000, 6000, 2500].iter().enumerate() {
        lines.push(format!(
            r#"{{"session_id":"s","prompt_tokens":{p},"completion_tokens":10,"cache_read_tokens":{},"context_length":10000,"compression_count":{},"archived_total":{}}}"#,
            p / 2,
            if i == 3 { 1 } else { 0 },
            if i == 3 { 7 } else { 0 }
        ));
    }
    std::fs::write(&log, lines.join("\n")).unwrap();
    let h = kritic::usage::hermes_usage_from(&log, None).unwrap();
    assert_eq!(
        (
            h.session_id.as_str(),
            h.api_calls,
            h.context_now,
            h.context_peak
        ),
        ("s", 4, 2500, 6000)
    );
    assert_eq!(
        (h.compactions, h.archived_messages, h.output_tokens),
        (1, 7, 40)
    );
    assert_eq!(h.context_utilization_pct, 25.0);
    assert_eq!(h.context_growth_per_call, 500.0);
    assert_eq!(
        kritic::usage::hermes_usage_from(&log, Some("old"))
            .unwrap()
            .api_calls,
        1
    );
    assert!(kritic::usage::hermes_usage_from(&log, Some("none")).is_none());
}
