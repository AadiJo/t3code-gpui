//! One environment's RPC session as the panel sources use it: each call runs on tokio (the
//! session needs its reactor) and resolves wherever it is awaited, so a source method is
//! `link.call::<Method>(payload)` mapped into its trait's error type.

use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _};
use gpui_kit::{App, SharedString};
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_client_runtime::rpc::RpcSession;
use t3code_client_runtime::rpc::client::RpcError;
use t3code_contracts::EnvironmentId;
use t3code_contracts::rpc::{RpcMethod, RpcStream};

use crate::connection::ConnectionRuntime;

/// How long a call waits for a session before it fails as not connected.
const SESSION_WAIT: Duration = Duration::from_secs(10);

/// Values a stream may hold before its forwarder stops reading (and acking) the socket.
const STREAM_BUFFER: usize = 8;

/// An environment's supervisor and the tokio runtime its calls run on. Cheap to clone.
#[derive(Clone)]
pub struct RpcLink {
    pub link: EnvironmentLink,
    pub tokio: tokio::runtime::Handle,
}

/// A failed call's message: the server's own for typed failures, the transport's otherwise.
pub fn error_message<E: std::fmt::Debug>(error: &RpcError<E>) -> SharedString {
    match error {
        RpcError::Failure(failure) => {
            let debug = format!("{failure:?}");
            let message = debug
                .split_once("message: \"")
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(message, _)| message.to_owned())
                .unwrap_or(debug);
            message.into()
        }
        other => other.to_string().into(),
    }
}

impl RpcLink {
    /// The link of a saved environment, once the connection runtime knows it.
    pub fn new(environment_id: &EnvironmentId, cx: &App) -> Option<Self> {
        let runtime = ConnectionRuntime::global(cx);
        Some(Self { link: runtime.environment_link(environment_id)?, tokio: runtime.handle().clone() })
    }

    pub fn environment_id(&self) -> &EnvironmentId {
        &self.link.environment_id
    }

    /// The live session, waiting up to [`SESSION_WAIT`] for one.
    pub async fn session(&self) -> Result<std::sync::Arc<RpcSession>, SharedString> {
        self.link.session(SESSION_WAIT).await.map_err(|error| SharedString::from(error.to_string()))
    }

    /// Runs `future` on tokio and resolves with its output anywhere.
    pub fn run<T: Send + 'static>(
        &self,
        future: impl std::future::Future<Output = Result<T, SharedString>> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, SharedString>> {
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.tokio.spawn(async move {
            _ = sender.send(future.await);
        });
        async move { receiver.await.unwrap_or_else(|_| Err("The app is shutting down.".into())) }.boxed()
    }

    /// One unary RPC.
    pub fn call<M: RpcMethod + 'static>(&self, payload: M::Payload) -> BoxFuture<'static, Result<M::Success, SharedString>>
    where
        M::Error: std::fmt::Debug,
    {
        let this = self.clone();
        self.run(async move {
            let session = this.session().await?;
            session.client.call::<M>(payload).await.map_err(|error| error_message(&error))
        })
    }

    /// One unary RPC whose typed failure `describe` words (an error class's own `message`); a
    /// failure it returns `None` for reads as [`error_message`] does.
    pub fn call_with<M: RpcMethod + 'static>(
        &self,
        payload: M::Payload,
        describe: impl FnOnce(&M::Error) -> Option<String> + Send + 'static,
    ) -> BoxFuture<'static, Result<M::Success, SharedString>>
    where
        M::Error: std::fmt::Debug,
    {
        let this = self.clone();
        self.run(async move {
            let session = this.session().await?;
            session.client.call::<M>(payload).await.map_err(|error| match &error {
                RpcError::Failure(failure) => describe(failure).map(SharedString::from).unwrap_or_else(|| error_message(&error)),
                _ => error_message(&error),
            })
        })
    }

    /// One streaming RPC, flattened to its values. The forwarder reads (and so acks) at most
    /// [`STREAM_BUFFER`] values ahead of the consumer.
    pub fn stream<M: RpcStream + 'static>(&self, payload: M::Payload) -> BoxStream<'static, Result<M::Success, SharedString>>
    where
        M::Error: std::fmt::Debug,
    {
        let (mut sender, receiver) = futures::channel::mpsc::channel(STREAM_BUFFER);
        let this = self.clone();
        self.tokio.spawn(async move {
            let session = match this.session().await {
                Ok(session) => session,
                Err(error) => {
                    _ = sender.send(Err(error)).await;
                    return;
                }
            };
            let mut subscription = session.client.subscribe::<M>(payload);
            while let Some(chunk) = subscription.next().await {
                match chunk {
                    Ok(values) => {
                        for value in values {
                            if sender.send(Ok(value)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        _ = sender.send(Err(error_message(&error))).await;
                        return;
                    }
                }
            }
        });
        receiver.boxed()
    }
}
