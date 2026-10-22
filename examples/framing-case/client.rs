// Synthetic bug: the client discards the canonical setting and uses its own constant.
pub fn client_timeout_ms(_canonical_ms: u64) -> u64 {
    1000
}
