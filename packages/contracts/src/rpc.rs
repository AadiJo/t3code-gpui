use serde::{Serialize, de::DeserializeOwned};

/// One RPC the server exposes: its wire tag and the encoded shapes of its
/// payload, success value, and typed error. The transport in
/// `t3code-client-runtime` is generic over this trait.
pub trait RpcMethod {
    const TAG: &'static str;
    type Payload: Serialize + Send + 'static;
    type Success: DeserializeOwned + Send + 'static;
    type Error: DeserializeOwned + Send + 'static;
}

/// Marks an RPC whose `Success` values arrive as stream chunks, ending with an
/// exit whose value is null.
pub trait RpcStream: RpcMethod {}
