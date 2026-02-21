use chrono::Utc;
use futures::Stream;
use reqwest::{redirect::Policy, Client, ClientBuilder};
use std::collections::HashMap;
use std::{env, pin::Pin, time::Duration};

use crate::{Link, LinkReader, LinkWriter};

pub struct HttpClientWrap<T> {
    client: Client,
    inner: T,
}

impl<T> HttpClientWrap<T> {
    pub fn new(client: Client, inner: T) -> Self {
        Self { client, inner }
    }

    pub fn wrap(inner: T) -> Self {
        let agent = concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION"),
            " (github.com/chrisdickinson/likelike)"
        );

        let max_redirects: usize = std::env::var("LIKELIKE_MAX_REDIRECTS")
            .ok()
            .and_then(|xs| xs.parse().ok())
            .unwrap_or(10);

        let timeout: u64 = std::env::var("LIKELIKE_REQUEST_TIMEOUT_SECONDS")
            .ok()
            .and_then(|xs| xs.parse().ok())
            .unwrap_or(15);

        let client = ClientBuilder::new()
            .redirect(Policy::limited(max_redirects))
            .user_agent(agent)
            .timeout(Duration::new(timeout, 0))
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .expect("default reqwest client could not be constructed");

        Self { inner, client }
    }
}

pub(crate) async fn fetch_link(mut link: Link, client: &Client) -> eyre::Result<Link> {
    if link.last_fetched.is_some() {
        eprintln!(
            "not fetching {}, last_fetched is {}",
            link.url(),
            link.last_fetched.unwrap_or_default()
        );
        return Ok(link);
    }

    let response = match client.get(link.url()).send().await {
        Ok(response) => response,
        Err(e) => {
            // We don't know if there's _really_ a problem if we can't connect: it could be our
            // local network or the site could be temporarily unavailable. We only really want
            // to throw up our hands if we're getting "oh no this site is complete garbage!"
            if e.is_connect() {
                return Ok(link);
            } else {
                return Err(e.into());
            }
        }
    };

    if !response.status().is_success() {
        return Ok(link);
    }

    link.last_fetched = Some(Utc::now());
    let http_headers = response
        .headers()
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.as_str().to_lowercase();
            if matches!(
                key.as_str(),
                "set-cookie" |
            "x-xss-protection" |
            "strict-transport-security" |
            "content-security-policy" |
            "x-content-security-policy" |
            "vary" |
            "referrer-policy" |
            "x-referrer-policy" |
            "x-frame-options" |
            "x-content-type-options" |
            "origin-trial" | // youtube
            "content-security-policy-report-only" | 
            "p3p" |
            "permissions-policy" |
            "report-to"
            ) {
                return None;
            }

            Some((key, value.to_str().ok()?.to_string()))
        })
        .fold(HashMap::new(), |mut acc, (key, value)| {
            acc.entry(key).or_insert_with(Vec::new).push(value);
            acc
        });

    let content_length: Option<usize> = http_headers
        .get("content-length")
        .and_then(|v| v.last())
        .into_iter()
        .find_map(|xs| xs.parse().ok());

    link.http_headers = Some(http_headers);

    if link.is_html() || link.is_pdf() || link.is_plaintext() {
        link.src = response.bytes().await.ok().map(|xs| xs.to_vec());
    } else {
        eprintln!("skipping link: {} {:?}", link.url(), link.http_headers().and_then(|hdrs| hdrs.get("content-type")).and_then(|xs| xs.last()).map(|xs| xs.as_str()));
    }

    Ok(link)
}

#[async_trait::async_trait]
impl<T: LinkReader + Send + Sync> LinkReader for HttpClientWrap<T> {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        self.inner.get(link).await
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a + Send>>> {
        self.inner.values().await
    }
}

#[async_trait::async_trait]
impl<T: LinkWriter + Send + Sync> LinkWriter for HttpClientWrap<T> {
    async fn write(&self, link: Link) -> eyre::Result<bool> {
        let link = fetch_link(link, &self.client).await?;
        self.inner.write(link).await
    }
}
