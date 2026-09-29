//! token_metrics transcript analysis on a synthetic transcript with known numbers.

use kritic::usage::analyze;
use serde_json::json;

fn line(v: serde_json::Value) -> String {
    v.to_string() + "\n"
}

fn assistant(id: &str, ts: &str, input: u64, cread: u64, cwrite: u64, out: u64) -> String {
    line(json!({"type": "assistant", "timestamp": ts, "isSidechain": false,
        "message": {"id": id, "model": "claude-opus-5-5", "usage": {"input_tokens": input, "cache_read_input_tokens": cread,
            "cache_creation_input_tokens": cwrite, "output_tokens": out, "output_tokens_details": {"thinking_tokens": out / 2}}}}))
}

#[test]
fn per_task_and_session_totals() {
    let mut t = String::new();
    t += &line(json!({"type": "user", "timestamp": "2026-09-27T10:00:00Z", "origin": {"kind": "human"}, "message": {"content": "first task please"}}));
    t += &assistant("m1", "2026-09-27T10:00:02Z", 10, 0, 5000, 200);
    // same message id repeated per content block: counted once, and the call ends at the last block
    t += &assistant("m1", "2026-09-27T10:00:04Z", 10, 0, 5000, 200);
    t += &line(json!({"type": "user", "timestamp": "2026-09-27T10:00:03Z", "message": {"content": [{"type": "tool_result"}]}}));
    t += &assistant("m2", "2026-09-27T10:00:07Z", 5, 5000, 800, 400);
    t += &line(json!({"type": "user", "timestamp": "2026-09-27T10:05:00Z", "origin": {"kind": "human"}, "message": {"content": [{"type": "text", "text": "second task"}]}}));
    t += &assistant("m3", "2026-09-27T10:05:10Z", 3, 5800, 100, 1000);
    t += &line(json!({"type": "system", "subtype": "compact_boundary", "timestamp": "2026-09-27T10:06:00Z"}));
    // synthetic entry without usage is ignored
    t += &line(json!({"type": "assistant", "timestamp": "2026-09-27T10:06:01Z", "message": {"id": "m4", "usage": {}}}));
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("sess.jsonl");
    std::fs::write(&p, t).unwrap();

    let (s, tasks, calls) = analyze(&p, 1_000_000).unwrap();
    assert_eq!(s.api_calls, 3);
    assert_eq!(calls.len(), 3);
    assert_eq!(s.tasks, 2);
    assert_eq!(s.output_tokens, 1600);
    assert_eq!(s.thinking_tokens, 800);
    assert_eq!(s.context_now, 3 + 5800 + 100);
    assert_eq!(s.context_peak, 5903);
    assert_eq!(s.compactions, 1);
    assert_eq!(s.processed_input_tokens, 5010 + 5805 + 5903);
    assert_eq!(tasks[0].api_calls, 2);
    assert_eq!(tasks[0].prompt, "first task please");
    assert_eq!(tasks[0].output_tokens, 600);
    assert_eq!((tasks[0].context_start, tasks[0].context_end), (5010, 5805));
    assert_eq!(tasks[1].prompt, "second task");
    // m3: 1000 output tokens over 10 s since the previous entry
    assert_eq!(tasks[1].output_tokens_per_s, Some(100.0));
    // m1: 200 tokens from 10:00:00 (prompt) to its last block at 10:00:04
    assert_eq!(calls[0].duration_ms, Some(4000));
    assert!((s.cache_hit_rate_pct - (10800.0 * 100.0 / 16718.0)).abs() < 0.1);
}
