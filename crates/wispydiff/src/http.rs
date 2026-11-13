//! GPUI's HTTP client (remote images in GitHub comments), on reqwest and the backend's runtime.

use std::sync::Arc;

use futures::future::BoxFuture;
use gpui_kit::http_client::{self, AsyncBody, HttpClient, Request, Response, Url};

pub struct ReqwestClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
    agent: http_client::http::HeaderValue,
}

impl ReqwestClient {
    pub fn new(runtime: tokio::runtime::Handle) -> Arc<Self> {
        Arc::new(ReqwestClient {
            client: reqwest::Client::builder().user_agent("WispyDiff").build().expect("HTTP client"),
            runtime,
            agent: http_client::http::HeaderValue::from_static("WispyDiff"),
        })
    }
}

impl HttpClient for ReqwestClient {
    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        Some(&self.agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, req: Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let (method, uri) = (req.method().as_str().to_string(), req.uri().to_string());
        let headers: Vec<(String, Vec<u8>)> = req.headers().iter().map(|(k, v)| (k.to_string(), v.as_bytes().to_vec())).collect();
        let task = self.runtime.spawn(async move {
            let method = reqwest::Method::from_bytes(method.as_bytes())?;
            let mut request = client.request(method, &uri);
            for (k, v) in headers {
                request = request.header(k, v);
            }
            let response = request.send().await?;
            let status = response.status().as_u16();
            let headers: Vec<(String, Vec<u8>)> = response.headers().iter().map(|(k, v)| (k.to_string(), v.as_bytes().to_vec())).collect();
            let body = response.bytes().await?.to_vec();
            anyhow::Ok((status, headers, body))
        });
        Box::pin(async move {
            let (status, headers, body) = task.await??;
            let mut builder = Response::builder().status(status);
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            Ok(builder.body(AsyncBody::from(body))?)
        })
    }
}
