//! Typed descriptions of the HTTP endpoints clients call on an environment server
//! (`EnvironmentHttpApi`) and on the T3 Connect relay (`RelayApi`), one zero-sized type per
//! endpoint in [`endpoints`].

use serde::{Serialize, de::DeserializeOwned};

/// The endpoint types, generated from the schema.
#[path = "generated/http_endpoints.rs"]
pub mod endpoints;

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    /// The method as written in a request line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// How an endpoint's request body is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyEncoding {
    /// No body.
    None,
    /// `application/json`.
    Json,
    /// `application/x-www-form-urlencoded`, from the body's fields.
    Form,
    /// `application/octet-stream`, the bytes as is.
    Bytes,
}

/// One HTTP endpoint: method, path template (`:name` segments come from `Params`), and the
/// encoded shapes of its parts. Headers are the ones the contract declares (for example
/// `authorization`); query and params encode as strings.
pub trait HttpEndpoint {
    const METHOD: HttpMethod;
    const PATH: &'static str;
    const BODY: BodyEncoding;
    /// The status of a successful response.
    const SUCCESS_STATUS: u16;
    type Params: Serialize;
    type Query: Serialize;
    type Headers: Serialize;
    type Body;
    /// The response body for `SUCCESS_STATUS`; `()` when there is none.
    type Success: DeserializeOwned;
    /// The decoded error body of any declared error status.
    type Error: DeserializeOwned;
}

/// Declares the endpoint types; used by the generated `http_endpoints.rs`.
macro_rules! http_endpoints {
    ($(
        $(#[$m:meta])*
        $name:ident {
            method: $method:ident,
            path: $path:literal,
            params: $params:ty,
            query: $query:ty,
            headers: $headers:ty,
            body: $body_kind:ident($body:ty),
            success: $status:literal => $success:ty,
            error: $error:ty,
        }
    )*) => {
        $(
            $(#[$m])*
            #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
            pub struct $name;

            impl $crate::http::HttpEndpoint for $name {
                const METHOD: $crate::http::HttpMethod = $crate::http::HttpMethod::$method;
                const PATH: &'static str = $path;
                const BODY: $crate::http::BodyEncoding = $crate::http::BodyEncoding::$body_kind;
                const SUCCESS_STATUS: u16 = $status;
                type Params = $params;
                type Query = $query;
                type Headers = $headers;
                type Body = $body;
                type Success = $success;
                type Error = $error;
            }
        )*
    };
}
pub(crate) use http_endpoints;
