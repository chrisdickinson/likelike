#![allow(dead_code)]
#![allow(unused_variables)]

use async_stream::stream;
use chrono::{TimeZone, Utc};
use futures::{stream, Stream};
use include_dir::{include_dir, Dir};
use reqwest::{header::HeaderMap, redirect::Policy, Client, ClientBuilder};
use sqlx::{Connection, SqliteConnection};
use std::{collections::HashMap, env, fmt::Debug, pin::Pin};
use tokio::sync::Mutex;

use crate::{FetchLinkMetadata, Link, ReadLinkInformation, WriteLinkInformation};

pub struct HttpClientWrap<T> {
    client: Client,
    inner: T,
}

impl<T> HttpClientWrap<T> {
    pub fn new(client: Client, inner: T) -> Self {
        Self { client, inner }
    }

    pub fn wrap(inner: T) -> Self {
        let agent = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

        let max_redirects: usize = std::env::var("LIKELIKE_MAX_REDIRECTS")
            .ok()
            .and_then(|xs| xs.parse().ok())
            .unwrap_or(10);

        let client = ClientBuilder::new()
            .redirect(Policy::limited(max_redirects))
            .user_agent(agent)
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .build()
            .expect("default reqwest client could not be constructed");

        Self { inner, client }
    }
}

#[async_trait::async_trait]
impl<T: Send + Sync> FetchLinkMetadata for HttpClientWrap<T> {
    type Headers = HeaderMap;
    type Body = Pin<Box<dyn Stream<Item = bytes::Bytes>>>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>> {
        let response = match self.client.get(link.url()).send().await {
            Ok(response) => response,
            Err(e) => {
                // We don't know if there's _really_ a problem if we can't connect: it could be our
                // local network or the site could be temporarily unavailable. We only really want
                // to throw up our hands if we're getting "oh no this site is complete garbage!"
                if e.is_connect() {
                    return Ok(None);
                } else {
                    return Err(e.into());
                }
            }
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        let headers = response.headers().clone();
        let body = response.bytes_stream();

        let stream = stream! {
            for await value in body {
                let Ok(value) = value else { continue };
                yield value
            }
        };

        Ok(Some((headers, Box::pin(stream))))
    }
}

#[async_trait::async_trait]
impl<T: ReadLinkInformation + Send + Sync> ReadLinkInformation for HttpClientWrap<T> {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        self.inner.get(link).await
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        self.inner.values().await
    }
}

#[async_trait::async_trait]
impl<T: WriteLinkInformation + Send + Sync> WriteLinkInformation for HttpClientWrap<T> {
    async fn update(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.update(link).await
    }

    async fn create(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.create(link).await
    }
}

pub struct DummyWrap<T> {
    inner: T,
}

impl<T> DummyWrap<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<T: Send + Sync> FetchLinkMetadata for DummyWrap<T> {
    type Headers = HeaderMap;
    type Body = Pin<Box<dyn Stream<Item = bytes::Bytes>>>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl<T: ReadLinkInformation + Send + Sync> ReadLinkInformation for DummyWrap<T> {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        self.inner.get(link).await
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        self.inner.values().await
    }
}

#[async_trait::async_trait]
impl<T: WriteLinkInformation + Send + Sync> WriteLinkInformation for DummyWrap<T> {
    async fn update(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.update(link).await
    }

    async fn create(&self, link: &Link) -> eyre::Result<bool> {
        self.inner.create(link).await
    }
}

/// An in-memory link store.
#[derive(Default, Debug)]
struct InMemoryStore {
    data: Mutex<HashMap<String, Link>>,
}

impl InMemoryStore {
    fn new() -> Self {
        Self {
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl ReadLinkInformation for InMemoryStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let data = self.data.lock().await;

        Ok(data.get(link).map(|l| l.clone()))
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let data = self.data.lock().await;
        let values: Vec<_> = data.values().cloned().collect();
        Ok(Box::pin(stream::iter(values.into_iter())))
    }
}

#[async_trait::async_trait]
impl WriteLinkInformation for InMemoryStore {
    async fn update(&self, link: &Link) -> eyre::Result<bool> {
        let mut data = self.data.lock().await;
        data.insert(link.url.clone(), link.clone());
        Ok(true)
    }

    async fn create(&self, link: &Link) -> eyre::Result<bool> {
        let mut data = self.data.lock().await;
        data.insert(link.url.clone(), link.clone());
        Ok(true)
    }
}

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[derive(Debug)]
pub struct SqliteStore {
    connection_string: String,
    sqlite: Mutex<SqliteConnection>,
}

impl SqliteStore {
    pub async fn new() -> Self {
        let env_var = env::var("LIKELIKE_DB");

        Self::with_connection_string(
            env_var
                .as_ref()
                .map(|s| s.as_str())
                .unwrap_or("sqlite::memory:"),
        )
        .await
        .unwrap()
    }

    pub async fn with_connection_string(s: impl AsRef<str>) -> eyre::Result<Self> {
        let s = s.as_ref();
        let mut sqlite = SqliteConnection::connect(s).await?;

        let mut files: Vec<_> = MIGRATIONS_DIR.files().collect();
        files.sort_by(|lhs, rhs| lhs.path().cmp(rhs.path()));
        for file in files {
            sqlx::query(unsafe { std::str::from_utf8_unchecked(file.contents()) })
                .execute(&mut sqlite)
                .await?;
        }

        Ok(Self {
            connection_string: s.to_string(),
            sqlite: Mutex::new(sqlite),
        })
    }
}

#[async_trait::async_trait]
impl WriteLinkInformation for SqliteStore {
    async fn update(&self, link: &Link) -> eyre::Result<bool> {
        let mut sqlite = self.sqlite.lock().await;
        let tags = serde_json::to_string(&link.tags)?;
        let via = serde_json::to_string(&link.via)?;
        let found_at = link.found_at.map(|xs| xs.timestamp_millis());
        let read_at = link.read_at.map(|xs| xs.timestamp_millis());

        let results = sqlx::query!(
            r#"
            UPDATE "links" SET
                title = ?,
                tags = json(?),
                via = ?,
                notes = ?,
                found_at = ?,
                read_at = ?,
                published_at = ?,
                from_filename = ?
            WHERE "url" = ?
            "#,
            link.title,
            tags,
            via,
            link.notes,
            found_at,
            read_at,
            link.published_at,
            link.from_filename,
            link.url
        )
        .execute(&mut *sqlite)
        .await?;

        Ok(results.rows_affected() > 0)
    }

    async fn create(&self, link: &Link) -> eyre::Result<bool> {
        let mut sqlite = self.sqlite.lock().await;
        let tags = serde_json::to_string(&link.tags)?;
        let via = serde_json::to_string(&link.via)?;

        let found_at = link.found_at.map(|xs| xs.timestamp_millis());
        let read_at = link.read_at.map(|xs| xs.timestamp_millis());

        let results = sqlx::query!(
            r#"
            INSERT INTO "links" (
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                from_filename,
                url
            ) VALUES (
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?
            )
            "#,
            link.title,
            tags,
            via,
            link.notes,
            found_at,
            read_at,
            link.from_filename,
            link.url
        )
        .execute(&mut *sqlite)
        .await?;

        Ok(results.rows_affected() > 0)
    }
}

#[async_trait::async_trait]
impl ReadLinkInformation for SqliteStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let mut sqlite = self.sqlite.lock().await;
        let Some(value) = sqlx::query!(
            r#"
            SELECT
                url,
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                published_at,
                from_filename
            FROM "links" WHERE "url" = ?"#,
            link
        )
        .fetch_optional(&mut *sqlite)
        .await? else { return Ok(None) };

        let found_at = value
            .found_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let read_at = value
            .read_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let published_at = value
            .published_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        Ok(Some(Link {
            url: value.url,
            title: value.title,
            tags: serde_json::from_str(&value.tags[..])?,
            via: value
                .via
                .and_then(|via| serde_json::from_str(&via[..]).ok()),
            notes: value.notes,
            found_at,
            read_at,
            published_at,
            from_filename: value.from_filename,
        }))
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let mut sqlite = self.sqlite.lock().await;

        let stream = stream! {
            let input = sqlx::query!(
                r#"
                SELECT
                    url,
                    title,
                    tags,
                    via,
                    notes,
                    found_at,
                    read_at,
                    published_at,
                    from_filename
                FROM "links"
                "#,
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };

                let found_at = value
                    .found_at
                    .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

                let read_at = value
                    .read_at
                    .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

                let published_at = value
                    .published_at
                    .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

                let Ok(tags) = serde_json::from_str(&value.tags[..]) else { continue };

                yield Link {
                    url: value.url,
                    title: value.title,
                    tags,
                    via: value
                        .via
                        .and_then(|via| serde_json::from_str(&via[..]).ok()),
                    notes: value.notes,
                    found_at,
                    read_at,
                    published_at,
                    from_filename: value.from_filename,
                }
            }
        };

        Ok(Box::pin(stream))
    }
}
