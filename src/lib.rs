#![allow(dead_code)]
#![allow(unused_variables)]

use async_stream::stream;
use chrono::{DateTime, TimeZone, Utc};
use comrak::{
    self,
    arena_tree::Node,
    nodes::{Ast, NodeLink, NodeValue},
    parse_document, Arena, ComrakOptions,
};
use futures::{stream, Stream, StreamExt};
use include_dir::{include_dir, Dir};
use sqlx::{Connection, SqliteConnection};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    env,
    fmt::Debug,
    pin::Pin,
};
use tokio::sync::{Mutex, MutexGuard};
#[async_trait::async_trait]
pub trait ReadLinkInformation {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>>;
    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>>;
}

#[async_trait::async_trait]
pub trait WriteLinkInformation: ReadLinkInformation {
    async fn insert(&self, mut link: Link) -> eyre::Result<bool> {
        if let Some(known_link) = self.get(link.url.as_str()).await? {
            // update
            link.read_at = known_link
                .read_at
                .or_else(|| link.notes.as_ref().map(|_| Utc::now()));

            link.found_at = known_link.found_at.or_else(|| Some(Utc::now()));

            self.update(&link).await
        } else {
            let now = Utc::now();
            link.found_at = Some(now);
            if link.notes.is_some() {
                link.read_at = Some(now);
            }

            self.create(&link).await
        }
    }

    async fn update(&self, link: &Link) -> eyre::Result<bool>;
    async fn create(&self, link: &Link) -> eyre::Result<bool>;
}

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
        let results = sqlx::query!(
            r#"
            UPDATE "links" SET
                title = ?,
                tags = json(?),
                via = ?,
                notes = ?,
                found_at = ?,
                read_at = ?
            WHERE "url" = ?
            "#,
            link.title,
            tags,
            via,
            link.notes,
            link.found_at,
            link.read_at,
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
        let results = sqlx::query!(
            r#"
            INSERT INTO "links" (
                title,
                tags,
                via,
                notes,
                found_at,
                read_at,
                url
            ) VALUES (
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
            link.found_at,
            link.read_at,
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
                read_at
            FROM "links" WHERE "url" = ?"#,
            link
        )
        .fetch_optional(&mut *sqlite)
        .await? else { return Ok(None) };

        let found_at = value
            .found_at
            .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

        let read_at = value
            .read_at
            .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

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
                    read_at
                FROM "links"
                "#,
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };

                let found_at = value
                    .found_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

                let read_at = value
                    .read_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

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
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

impl SqliteStore {
    async fn values(&self) -> eyre::Result<Pin<Box<impl Stream<Item = Link> + '_>>> {
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
                    read_at
                FROM "links"
                "#,
            )
            .fetch(&mut *sqlite);

            for await value in input {
                let Ok(value) = value else { continue };

                let found_at = value
                    .found_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

                let read_at = value
                    .read_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

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
                }
            }
        };

        Ok(Box::pin(stream))
    }
    fn entries(self) -> Pin<Box<impl Stream<Item = Link>>> {
        let mut sqlite = self.sqlite.into_inner();

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
                    read_at
                FROM "links"
                "#,
            )
            .fetch(&mut sqlite);

            for await value in input {
                let Ok(value) = value else { continue };

                let found_at = value
                    .found_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

                let read_at = value
                    .read_at
                    .and_then(|xs| Utc.timestamp_opt(xs, 0).latest());

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
                }
            }
        };

        Box::pin(stream)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub enum Via {
    Friend(String),
    Link(String),
    Freeform(String),
    #[default]
    Unknown,
}

#[allow(dead_code)]
#[derive(Default, Clone, Debug)]
pub struct Link {
    url: String,
    title: String,
    via: Option<Via>,
    tags: HashSet<String>,
    notes: Option<String>,
    found_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
}

pub async fn process_input<S, Store>(input: S, store: &Store) -> eyre::Result<()>
where
    S: AsRef<str>,
    Store: WriteLinkInformation + Send + Sync,
{
    let arena = Arena::new();
    let opts = ComrakOptions::default();

    let root = parse_document(&arena, input.as_ref(), &opts);

    let links = root
        .children()
        .filter(|c| matches!(c.data.borrow().value, NodeValue::List(_)))
        .map(|c| c.children())
        .flatten()
        .filter(|c| {
            matches!(
                c.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            )
        })
        .filter_map(|list_item_node| {
            let mut children = list_item_node.children().into_iter();

            if let Some(para) = children.next() {
                if !matches!(para.data.borrow().value, NodeValue::Paragraph) {
                    return None;
                }

                let Ok(mut link) = extract_link_from_paragraph(para) else {
                    return None;
                };

                for child in children {
                    if !matches!(child.data.borrow().value, NodeValue::List(_)) {
                        continue;
                    }

                    if let Err(_) = extract_metadata_from_child_list(&mut link, child) {
                        continue;
                    }
                }

                Some(link)
            } else {
                None
            }
        });

    for link in links {
        store.insert(link).await?;
    }

    Ok(())
}

fn extract_metadata_from_child_list<'a>(
    link: &mut Link,
    list: &'a Node<'a, RefCell<Ast>>,
) -> eyre::Result<()> {
    for list_item_node in list.children() {
        if !matches!(
            list_item_node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_)
        ) {
            continue;
        }

        // grab the paragraph from the first Item
        let mut list_item_children = list_item_node.children();
        let Some(first_child) = list_item_children.next() else { continue };
        if !matches!(first_child.data.borrow().value, NodeValue::Paragraph) {
            continue;
        }
        let Ok(first_child_text) = fmt_cmark(first_child) else { continue };

        // TODO: actually handle these things!
        match first_child_text.split(":").next() {
            Some("tags") => {
                let mut tags: HashSet<_> = (&first_child_text["tags:".len()..])
                    .trim()
                    .split(",")
                    .map(|xs| xs.trim().to_string())
                    .collect();

                if let Some(child) = list_item_children.next() {
                    if matches!(child.data.borrow().value, NodeValue::List(_)) {
                        for list_item in child.children() {
                            let Some(list_item_graf) = list_item.children().next() else { continue };
                            if !matches!(list_item_graf.data.borrow().value, NodeValue::Paragraph) {
                                continue;
                            }
                            let Ok(list_item_graf_text) = fmt_cmark(list_item_graf) else { continue };
                            tags.extend(
                                list_item_graf_text
                                    .trim()
                                    .split(",")
                                    .map(|xs| xs.trim().to_string()),
                            );
                        }
                    }
                }

                link.tags.extend(tags);
            }
            Some("via") => {
                link.via = Some(parse_via(&first_child_text["via:".len()..].trim()));
            }
            Some("notes") => {
                if let Some(child) = list_item_children.next() {
                    if !matches!(child.data.borrow().value, NodeValue::List(_)) {
                        continue;
                    }

                    let mut notes: String = child
                        .children()
                        .map(|list_item| list_item.children())
                        .flatten()
                        .filter_map(|node| fmt_cmark(node).ok())
                        .collect();

                    if let Some(prior_notes) = &link.notes {
                        notes = prior_notes.to_string() + &notes;
                    }

                    link.notes = Some(notes);
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn parse_via(text: &str) -> Via {
    if &text[..1] == "@" {
        return Via::Friend(text.trim().to_string());
    }

    match text.split(':').next() {
        Some("http") | Some("https") => Via::Link(text.trim().to_string()),
        _ => Via::Freeform(text.to_string()),
    }
}

fn extract_link_from_paragraph<'a>(graf: &'a Node<'a, RefCell<Ast>>) -> eyre::Result<Link> {
    for child in graf.children() {
        let NodeValue::Link(NodeLink { ref url, ref title }) = child.data.borrow().value else {
            continue
        };

        let Ok(url) = std::str::from_utf8(url) else {
            continue
        };

        let Ok(title) = std::str::from_utf8(title) else {
            continue
        };

        return Ok(Link {
            url: url.to_string(),
            title: title.to_string(),
            ..Default::default()
        });
    }

    let text = fmt_cmark(graf)?;
    let mut pieces = text.split(&['-', ':'][..]);
    Ok(match pieces.next() {
        Some("https") | Some("http") => Link {
            title: String::new(),
            url: text.trim().to_string(),
            ..Default::default()
        },

        Some(t) => Link {
            title: text.to_string(),
            url: text[t.len() + 1..].trim().to_string(),
            ..Default::default()
        },

        None => return Err(eyre::eyre!("empty paragraph, no link")),
    })
}

fn fmt_cmark<'a>(node: &'a Node<'a, RefCell<Ast>>) -> eyre::Result<String> {
    let mut output = Vec::with_capacity(512);
    comrak::format_commonmark(node, &ComrakOptions::default(), &mut output)?;
    String::from_utf8(output).map_err(|e| e.into())
}

#[cfg(test)]
mod tests {
    use futures::pin_mut;

    use super::*;

    #[tokio::test]
    async fn it_works() -> eyre::Result<()> {
        let store = SqliteStore::new().await;

        process_input(
            r#"
# just plain links

- plain text title w/link: https://foo.bar/baz 
- https://bar.dev/baz 
- [single url](https://google.com/)
- [single url with title](https://apple.com/ "title")
- some *markdown*: https://foo.baz

# read links

- read link *markdown*: https://foo.baz
    - via: @garybusey
    - tags: alpha, beta
        - gamma
        - epsilon, mu
- read link *other markdown*: https://foo.baz
    - tags: foo, bar, baz
    - via: https://bar.dev/baz
    - notes:
        - # just testing
        - wow, so interesting
    - notes:
        - ```rust
          fn main() -> {}
          ```
        - hello world
"#,
            &store,
        )
        .await?;

        let mut s = store.values().await?;

        while let Some(v) = s.next().await {
            dbg!(v);
        }

        Ok(())
    }
}
