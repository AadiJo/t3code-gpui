//! Effect RPC wire envelopes over the `/ws` socket (spec 01, section 3).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// The client pings this often.
pub const PING_INTERVAL: Duration = Duration::from_secs(5);
/// The socket is dropped after this long without any frame, parts included.
pub const PING_TIMEOUT: Duration = Duration::from_secs(15);
/// Opening the socket (TCP, TLS, upgrade) must finish within this.
pub const SOCKET_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// Query key that opts in to split server frames.
pub const FRAME_PARTS_QUERY_PARAM: &str = "frameParts";
/// Marks every part of a split frame except the last.
const PART_PREFIX: char = '\u{1e}';

/// Per-socket request id, sent as a JSON number.
pub type RequestId = u64;

/// Envelopes the client sends.
#[derive(Serialize, Debug)]
#[serde(tag = "_tag")]
pub enum ClientMessage<'a> {
    Request {
        id: RequestId,
        tag: &'static str,
        payload: &'a RawValue,
        headers: [(); 0],
    },
    Ack {
        #[serde(rename = "requestId")]
        request_id: RequestId,
    },
    Interrupt {
        #[serde(rename = "requestId")]
        request_id: RequestId,
    },
    Ping,
}

/// One server envelope, borrowed from its frame. A plain struct, not an
/// enum, because serde cannot buffer `RawValue` inside a tagged enum.
#[derive(Deserialize, Debug)]
pub struct ServerEnvelope<'a> {
    #[serde(rename = "_tag", borrow)]
    pub tag: std::borrow::Cow<'a, str>,
    #[serde(rename = "requestId", default)]
    pub request_id: Option<WireRequestId>,
    #[serde(borrow, default)]
    pub values: Option<Vec<&'a RawValue>>,
    #[serde(borrow, default)]
    pub exit: Option<&'a RawValue>,
    #[serde(borrow, default)]
    pub defect: Option<&'a RawValue>,
}

/// The server echoes ids unchanged; this client only sends numbers, so a
/// string id is not ours.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(untagged)]
pub enum WireRequestId {
    Number(RequestId),
    #[serde(deserialize_with = "ignore_string")]
    Foreign,
}

fn ignore_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| ())
}

impl WireRequestId {
    pub fn ours(self) -> Option<RequestId> {
        match self {
            Self::Number(id) => Some(id),
            Self::Foreign => None,
        }
    }
}

/// Decodes one whole frame: a single envelope or a JSON array of them.
pub fn decode_frame(frame: &str) -> Result<Vec<ServerEnvelope<'_>>, serde_json::Error> {
    if frame.trim_start().starts_with('[') {
        serde_json::from_str(frame)
    } else {
        serde_json::from_str(frame).map(|envelope| vec![envelope])
    }
}

/// `Exit.exit`: `{"_tag":"Success","value":..}` or `{"_tag":"Failure","cause":[..]}`.
#[derive(Deserialize, Debug)]
pub struct ExitEncoded<'a> {
    #[serde(rename = "_tag", borrow)]
    pub tag: std::borrow::Cow<'a, str>,
    #[serde(borrow, default)]
    pub value: Option<&'a RawValue>,
    /// Effect encodes a cause as an array of reasons (seen on the wire:
    /// `[{"_tag":"Interrupt","fiberId":2064}]`).
    #[serde(borrow, default)]
    pub cause: Vec<CauseReason<'a>>,
}

/// One cause reason: `Fail` (typed error), `Die` (defect) or `Interrupt`.
/// `fiberId` is ignored, absent, null or a number.
#[derive(Deserialize, Debug)]
pub struct CauseReason<'a> {
    #[serde(rename = "_tag", borrow)]
    pub tag: std::borrow::Cow<'a, str>,
    #[serde(borrow, default)]
    pub error: Option<&'a RawValue>,
    #[serde(borrow, default)]
    pub defect: Option<&'a RawValue>,
}

/// What an `Exit` meant, before the typed error is decoded.
pub enum ExitKind<'a> {
    Success(Option<&'a RawValue>),
    /// The first `Fail` reason's encoded error.
    Fail(&'a RawValue),
    /// The first `Die` reason's encoded defect.
    Die(&'a RawValue),
    Interrupted,
}

impl<'a> ExitEncoded<'a> {
    /// The first `Fail`, else the first `Die`, else interrupted.
    pub fn kind(&self) -> ExitKind<'a> {
        if self.tag == "Success" {
            return ExitKind::Success(self.value);
        }
        let reasons = &self.cause;
        if let Some(error) = reasons.iter().find(|r| r.tag == "Fail").and_then(|r| r.error) {
            return ExitKind::Fail(error);
        }
        if let Some(defect) = reasons.iter().find(|r| r.tag == "Die").and_then(|r| r.defect) {
            return ExitKind::Die(defect);
        }
        ExitKind::Interrupted
    }
}

/// Reassembles frames the server split with `frameParts=1`.
#[derive(Default)]
pub struct FrameJoiner {
    pending: String,
}

impl FrameJoiner {
    /// Feeds one text frame. Returns the whole message once its last part
    /// arrives, or `None` while parts are still held.
    pub fn push(&mut self, frame: String) -> Option<String> {
        if let Some(part) = frame.strip_prefix(PART_PREFIX) {
            self.pending.push_str(part);
            return None;
        }
        if self.pending.is_empty() {
            return Some(frame);
        }
        let mut whole = std::mem::take(&mut self.pending);
        whole.push_str(&frame);
        Some(whole)
    }

    /// Bytes held from parts whose last part has not arrived.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
