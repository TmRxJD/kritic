use kritic_prompts::codex::decode_codex;

#[test]
fn completed_native_turn_returns_its_json_proposal() {
    let events=b"{\"type\":\"thread.started\",\"thread_id\":\"fresh\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"action\\\":\\\"keep\\\"}\"}}\n{\"type\":\"turn.completed\"}\n";
    assert_eq!(decode_codex(events).unwrap()["action"], "keep");
}

#[test]
fn missing_completion_failed_turn_tool_item_and_invalid_json_are_rejected() {
    for events in [b"{\"type\":\"turn.started\"}\n".as_slice(),
        b"{\"type\":\"turn.failed\"}\n",
        b"{\"type\":\"item.completed\",\"item\":{\"type\":\"mcp_tool_call\"}}\n{\"type\":\"turn.completed\"}\n",
        b"not json\n"] {
        assert!(decode_codex(events).is_err());
    }
}
