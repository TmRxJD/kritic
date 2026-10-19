//! Framed byte protocol between the runner and a game client (or an external policy process).
//! Frame: `u32 LE length` (of kind + payload) | `u8 kind` | payload. See docs/policy-runner.md.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest accepted frame (kind + payload).
pub const MAX_FRAME: usize = 16 << 20;

pub const HELLO: u8 = 0x01;
pub const RESET: u8 = 0x02;
pub const STATE: u8 = 0x03;
pub const ACTION: u8 = 0x04;
pub const BYE: u8 = 0x05;
pub const ERROR: u8 = 0x7F;

/// STATE flag: the episode ended at this state.
pub const FLAG_TERMINAL: u8 = 0x01;

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// client -> runner: UTF-8 JSON `{"schema": name, "version": v, "state_bytes": n}`.
    Hello(serde_json::Value),
    /// runner -> client: start an episode.
    Reset { seed: u64, max_ticks: u32 },
    /// client -> runner: the state at `tick`; `terminal` ends the episode.
    State {
        tick: u64,
        terminal: bool,
        state: Vec<u8>,
    },
    /// runner -> client: the action for `tick`. Empty `action` = defer to the game's own policy.
    Action { tick: u64, action: Vec<u8> },
    /// runner -> client: no more episodes.
    Bye,
    /// either way: a fatal error message.
    Error(String),
}

impl Msg {
    pub fn encode(&self) -> (u8, Vec<u8>) {
        match self {
            Msg::Hello(v) => (HELLO, v.to_string().into_bytes()),
            Msg::Reset { seed, max_ticks } => {
                let mut p = seed.to_le_bytes().to_vec();
                p.extend_from_slice(&max_ticks.to_le_bytes());
                (RESET, p)
            }
            Msg::State {
                tick,
                terminal,
                state,
            } => {
                let mut p = tick.to_le_bytes().to_vec();
                p.push(if *terminal { FLAG_TERMINAL } else { 0 });
                p.extend_from_slice(state);
                (STATE, p)
            }
            Msg::Action { tick, action } => {
                let mut p = tick.to_le_bytes().to_vec();
                p.extend_from_slice(action);
                (ACTION, p)
            }
            Msg::Bye => (BYE, Vec::new()),
            Msg::Error(m) => (ERROR, m.as_bytes().to_vec()),
        }
    }

    pub fn decode(kind: u8, p: Vec<u8>) -> Result<Msg, String> {
        let u64_at = |p: &[u8]| -> Result<u64, String> {
            p.get(..8)
                .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
                .ok_or_else(|| format!("frame 0x{kind:02x} too short"))
        };
        Ok(match kind {
            HELLO => Msg::Hello(
                serde_json::from_slice(&p).map_err(|e| format!("HELLO is not JSON: {e}"))?,
            ),
            RESET => {
                if p.len() != 12 {
                    return Err("RESET must be 12 bytes".into());
                }
                Msg::Reset {
                    seed: u64_at(&p)?,
                    max_ticks: u32::from_le_bytes(p[8..12].try_into().expect("4 bytes")),
                }
            }
            STATE => {
                if p.len() < 9 {
                    return Err("STATE shorter than its 9-byte header".into());
                }
                Msg::State {
                    tick: u64_at(&p)?,
                    terminal: p[8] & FLAG_TERMINAL != 0,
                    state: p[9..].to_vec(),
                }
            }
            ACTION => Msg::Action {
                tick: u64_at(&p)?,
                action: p[8..].to_vec(),
            },
            BYE => Msg::Bye,
            ERROR => Msg::Error(String::from_utf8_lossy(&p).into_owned()),
            k => return Err(format!("unknown frame kind 0x{k:02x}")),
        })
    }
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, m: &Msg) -> Result<(), String> {
    let (kind, payload) = m.encode();
    let len = (payload.len() + 1) as u32;
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(&payload);
    w.write_all(&buf).await.map_err(|e| format!("write: {e}"))?;
    w.flush().await.map_err(|e| format!("flush: {e}"))
}

pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg, String> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(format!("bad frame length {len}"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let kind = body[0];
    body.remove(0);
    Msg::decode(kind, body)
}
