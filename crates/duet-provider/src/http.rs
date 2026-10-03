// SPDX-License-Identifier: GPL-3.0-or-later
//! The only production HTTP-client constructor in this crate. Callers get a
//! finished client, never a builder on which they can loosen proxy/redirect
//! policy. `tools/gate.sh` rejects constructors anywhere else in the provider.

use crate::endpoint::ApprovedEndpoint;
use crate::{ErrorKind, ProviderError};
use std::time::Duration;

pub(crate) fn client(
    endpoint: &ApprovedEndpoint,
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
) -> Result<Client, reqwest::Error> {
    let builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    let inner = match request_timeout {
        Some(timeout) => builder.timeout(timeout),
        None => builder,
    }
    .build()?;
    Ok(Client {
        inner,
        endpoint: endpoint.clone(),
    })
}

/// Never exposes the raw client; every request retains its approved recipient.
pub(crate) struct Client {
    inner: reqwest::Client,
    endpoint: ApprovedEndpoint,
}
impl Client {
    fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        if !self.endpoint.permits(url) {
            return Err(ProviderError::new(
                ErrorKind::Forbidden,
                "request recipient differs from the approved model endpoint",
            ));
        }
        Ok(self.inner.request(method, url))
    }
    pub(crate) fn get(
        &self,
        url: impl AsRef<str>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        self.request(reqwest::Method::GET, url.as_ref())
    }
    pub(crate) fn post(
        &self,
        url: impl AsRef<str>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        self.request(reqwest::Method::POST, url.as_ref())
    }
}
