//! The RPC methods of `WsRpcGroup`: one zero-sized type per method in [`methods`], each
//! implementing [`RpcMethod`] (and [`RpcStream`] for the 30 streaming methods).

use crate::AuthEnvironmentScope;
use serde::{Serialize, de::DeserializeOwned};

/// The method types, generated from the schema.
#[path = "generated/rpc_methods.rs"]
pub mod methods;

/// One RPC the server exposes: its wire tag, the scope the server requires, and the encoded
/// shapes of its payload, success value and typed error. The transport in
/// `t3code-client-runtime` is generic over this trait.
pub trait RpcMethod {
    /// The method tag sent in a `Request` frame.
    const TAG: &'static str;
    /// The scope the server checks on every call (`RPC_REQUIRED_SCOPES`). Some methods need more
    /// depending on the payload; the server stays authoritative.
    const SCOPE: AuthEnvironmentScope;
    /// Whether clients hide this action from a session without `SCOPE`
    /// (`CLIENT_GUARDED_RPC_SCOPES`).
    const CLIENT_GUARDED: bool;
    type Payload: Serialize + Send + 'static;
    /// The `Exit` value of a unary call, or one `Chunk` element of a stream.
    type Success: DeserializeOwned + Send + 'static;
    type Error: DeserializeOwned + Send + 'static;
}

/// Marks an RPC whose `Success` values arrive as stream chunks, ending with an exit whose value
/// is null.
pub trait RpcStream: RpcMethod {}

/// Declares the method types; used by the generated `rpc_methods.rs`.
macro_rules! rpc_methods {
    ($(
        $(#[$m:meta])*
        $name:ident {
            tag: $tag:literal,
            scope: $scope:ident,
            client_guarded: $guarded:literal,
            payload: $payload:ty,
            success: $success:ty,
            error: $error:ty,
            stream: $stream:tt,
        }
    )*) => {
        $(
            $(#[$m])*
            #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
            pub struct $name;

            impl $crate::rpc::RpcMethod for $name {
                const TAG: &'static str = $tag;
                const SCOPE: $crate::AuthEnvironmentScope = $crate::AuthEnvironmentScope::$scope;
                const CLIENT_GUARDED: bool = $guarded;
                type Payload = $payload;
                type Success = $success;
                type Error = $error;
            }

            $crate::rpc::rpc_methods!(@stream $stream $name);
        )*

        /// Every method tag, in schema order.
        pub const ALL_TAGS: &[&str] = &[$($tag,)*];
    };
    (@stream true $name:ident) => {
        impl $crate::rpc::RpcStream for $name {}
    };
    (@stream false $name:ident) => {};
}
pub(crate) use rpc_methods;
