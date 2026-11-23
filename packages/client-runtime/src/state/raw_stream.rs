//! A stream RPC whose values arrive as raw JSON, for streams the state layer decodes itself
//! (thread snapshots carry an id table that must be expanded before decoding).

use std::marker::PhantomData;

use serde_json::value::RawValue;
use t3code_contracts::AuthEnvironmentScope;
use t3code_contracts::rpc::{RpcMethod, RpcStream};

/// `M` with undecoded success values.
pub struct RawStream<M>(PhantomData<fn() -> M>);

impl<M: RpcStream> RpcMethod for RawStream<M> {
    const TAG: &'static str = M::TAG;
    const SCOPE: AuthEnvironmentScope = M::SCOPE;
    const CLIENT_GUARDED: bool = M::CLIENT_GUARDED;
    type Payload = M::Payload;
    type Success = Box<RawValue>;
    type Error = M::Error;
}

impl<M: RpcStream> RpcStream for RawStream<M> {}
