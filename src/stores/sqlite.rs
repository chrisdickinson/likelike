use async_stream::stream;
use chrono::{TimeZone, Utc};
use futures::Stream;
use include_dir::{include_dir, Dir};

use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Row, SqliteConnection};
use std::{env, fmt::Debug, pin::Pin, str::FromStr};
use tokio::sync::Mutex;

use crate::{Link, LinkReader, LinkWriter};

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[derive(Debug)]
pub struct SqliteStore {
    sqlite: Mutex<SqliteConnection>,
}

impl SqliteStore {
    pub async fn new() -> Self {
        let env_var = env::var("LIKELIKE_DB");

        Self::with_connection_string(if let Ok(db_url) = env_var {
            db_url
        } else {
            let location = dirs::data_local_dir()
                .map(|mut xs| {
                    xs.push("likelike");
                    std::fs::create_dir_all(&xs).expect("Must be able to create XDG_SHARE_HOME");
                    xs.push("db.sqlite3");
                    xs.to_string_lossy().to_string()
                })
                .unwrap_or_else(|| ":memory:".to_string());

            format!("sqlite://{}", location)
        })
        .await
        .unwrap()
    }

    pub async fn with_connection_string(s: impl AsRef<str>) -> eyre::Result<Self> {
        Self::with_connection_options(SqliteConnectOptions::from_str(s.as_ref())?).await
    }

    pub async fn with_connection_options(opts: SqliteConnectOptions) -> eyre::Result<Self> {
        let mut sqlite = opts.create_if_missing(true).connect().await?;

        let mut files: Vec<_> = MIGRATIONS_DIR.files().collect();
        files.sort_by(|lhs, rhs| lhs.path().cmp(rhs.path()));
        let version = files.len();

        let last_index = sqlx::query("select version from database_version limit 1")
            .fetch_one(&mut sqlite)
            .await
            .map(|result| result.get("version"))
            .unwrap_or_else(|_| 0u32);

        let migration_count = files.len() as u32;
        for file in files.into_iter().skip(last_index as usize) {
            sqlx::query(unsafe { std::str::from_utf8_unchecked(file.contents()) })
                .execute(&mut sqlite)
                .await?;
        }

        sqlx::query(r#"
            insert into database_version (id, version) values (0, ?) on conflict(id) do update set version = excluded.version
        "#).bind(migration_count).execute(&mut sqlite)
            .await?;

        Ok(Self {
            sqlite: Mutex::new(sqlite),
        })
    }
}

#[async_trait::async_trait]
impl LinkWriter for SqliteStore {
    async fn write(&self, link: Link) -> eyre::Result<bool> {
        let mut sqlite = self.sqlite.lock().await;
        let tags = serde_json::to_string(&link.tags)?;
        let via = serde_json::to_string(&link.via)?;

        let found_at = link.found_at.map(|xs| xs.timestamp_millis());
        let read_at = link.read_at.map(|xs| xs.timestamp_millis());

        let last_fetched = link.last_fetched.map(|xs| xs.timestamp_millis());
        let last_processed = link.last_processed.map(|xs| xs.timestamp_millis());

        let src = link
            .src
            .iter()
            .filter_map(|src| zstd::encode_all(src.as_slice(), 0).ok())
            .next();

        let meta = link
            .meta
            .iter()
            .filter_map(|src| serde_json::to_vec(src).ok())
            .next();

        let http_headers = link
            .http_headers
            .iter()
            .filter_map(|http_headers| serde_json::to_vec(http_headers).ok())
            .filter_map(|src| zstd::encode_all(src.as_slice(), 0).ok())
            .next();

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
                url,
                image,
                src,
                meta,
                last_fetched,
                last_processed,
                http_headers
            ) VALUES (
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?,
                ?
            ) ON CONFLICT (url) DO UPDATE
                SET title=excluded.title,
                    tags=excluded.tags,
                    via=excluded.via,
                    notes=excluded.notes,
                    found_at=excluded.found_at,
                    read_at=excluded.read_at,
                    from_filename=excluded.from_filename,
                    image=excluded.image,
                    src=excluded.src,
                    meta=excluded.meta,
                    last_fetched=excluded.last_fetched,
                    last_processed=excluded.last_processed,
                    http_headers=excluded.http_headers
            "#,
            link.title,
            tags,
            via,
            link.notes,
            found_at,
            read_at,
            link.from_filename,
            link.url,
            link.image,
            src,
            meta,
            last_fetched,
            last_processed,
            http_headers
        )
        .execute(&mut *sqlite)
        .await?;

        Ok(results.rows_affected() > 0)
    }
}

struct LinkRow {
    url: String,
    title: Option<String>,
    tags: String,
    via: Option<String>,
    notes: Option<String>,
    found_at: Option<i64>,
    read_at: Option<i64>,
    published_at: Option<i64>,
    from_filename: Option<String>,
    image: Option<String>,
    src: Option<Vec<u8>>,
    meta: Option<String>,
    last_fetched: Option<i64>,
    last_processed: Option<i64>,
    http_headers: Option<Vec<u8>>,
}

impl TryFrom<LinkRow> for Link {
    type Error = eyre::Error;

    fn try_from(value: LinkRow) -> Result<Self, Self::Error> {
        let found_at = value
            .found_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let read_at = value
            .read_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let published_at = value
            .published_at
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let last_fetched = value
            .last_fetched
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let last_processed = value
            .last_processed
            .and_then(|xs| Utc.timestamp_millis_opt(xs).latest());

        let meta = value
            .meta
            .iter()
            .find_map(|src| serde_json::from_str(src).ok());

        let src = value
            .src
            .iter()
            .find_map(|src| zstd::decode_all(src.as_slice()).ok())
            .map(Into::into);

        let http_headers = value
            .http_headers
            .iter()
            .filter_map(|src| zstd::decode_all(src.as_slice()).ok())
            .find_map(|src| serde_json::from_slice(src.as_slice()).ok());

        Ok(Link {
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
            image: value.image,
            src,
            meta,
            last_fetched,
            last_processed,
            http_headers,
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl LinkReader for SqliteStore {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        let mut sqlite = self.sqlite.lock().await;
        let Some(value) = sqlx::query_as!(
            LinkRow,
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
                from_filename,
                image,
                src,
                meta,
                last_fetched,
                last_processed,
                http_headers
            FROM "links" WHERE "url" = ?"#,
            link
        )
        .fetch_optional(&mut *sqlite)
        .await? else { return Ok(None) };

        Ok(Some(value.try_into()?))
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let mut sqlite = self.sqlite.lock().await;

        let stream = stream! {
            let input = sqlx::query_as!(
                LinkRow,
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
                    from_filename,
                    image,
                    NULL as "src?: Vec<u8>", -- explicitly DO NOT FETCH the source data
                    meta,
                    last_fetched,
                    last_processed,
                    http_headers
                FROM "links"
                "#,
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };
                let Ok(link) = value.try_into() else { continue };

                yield link
            }
        };

        Ok(Box::pin(stream))
    }

    async fn glob<'a, 'b: 'a>(
        &'a self,
        pattern: &'b str,
    ) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let mut sqlite = self.sqlite.lock().await;

        let stream = stream! {
            let input = sqlx::query_as!(
                LinkRow,
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
                    from_filename,
                    image,
                    src,
                    meta,
                    last_fetched,
                    last_processed,
                    http_headers
                FROM "links"
                WHERE url GLOB ?
                "#,
                pattern
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };
                let Ok(link) = value.try_into() else { continue };

                yield link
            }
        };

        Ok(Box::pin(stream))
    }
}
