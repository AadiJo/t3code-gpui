use url::Url;

/// An endpoint on the environment's origin: `path` replaces the base URL's
/// path, query and fragment. Servers under a path prefix are not supported.
pub fn endpoint_url(http_base_url: &Url, path: &str) -> Url {
    let mut url = http_base_url.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// The RPC socket URL: `/ws` when the base URL has no path of its own.
pub fn rpc_socket_url(ws_base_url: &Url) -> Url {
    let mut url = ws_base_url.clone();
    if url.path().is_empty() || url.path() == "/" {
        url.set_path("/ws");
    }
    url
}
