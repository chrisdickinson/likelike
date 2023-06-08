#![allow(dead_code)]
#![allow(unused_variables)]

use comrak::{
    self,
    arena_tree::Node,
    nodes::{Ast, NodeLink, NodeValue},
    parse_document, Arena, ComrakOptions,
};
use futures::Stream;

use reqwest::header::HeaderMap;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    pin::Pin,
};

mod domain;
mod enrichment;
mod stores;

pub use crate::domain::*;
pub use crate::stores::*;

/// Read link information from the link store.
#[async_trait::async_trait]
pub trait ReadLinkInformation {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>>;
    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>>;
    async fn glob<'a, 'b: 'a>(&'a self, pattern: &'b str) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let m = wildmatch::WildMatch::new(pattern);
        let values = self.values().await?;

        Ok(Box::pin(async_stream::stream! {
            futures::pin_mut!(values);
            for await link in values {
                if m.matches(link.url.as_str()) {
                    let Ok(Some(link)) = self.get(link.url.as_str()).await else { continue };
                    yield link;
                }
            }
        }))
    }
}

/// Write link information back to the link store.
#[async_trait::async_trait]
pub trait WriteLinkInformation {
    async fn write(&self, link: &Link) -> eyre::Result<bool>;
}

#[async_trait::async_trait]
pub trait FetchLinkMetadata {
    type Headers: TryInto<HeaderMap>;
    type Body: Stream<Item = bytes::Bytes>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>>;
}

pub async fn process_input<'a, S, Store>(input: S, store: &Store) -> eyre::Result<()>
where
    S: Into<LinkSource<'a>> + Send + Sync,
    Store: FetchLinkMetadata + ReadLinkInformation + WriteLinkInformation + Send + Sync,
{
    let arena = Arena::new();
    let opts = ComrakOptions::default();

    let link_source = input.into();

    let root = parse_document(&arena, link_source.content.as_ref(), &opts);

    let mut links = HashMap::new();

    let iter_list_items = root
        .children()
        .filter(|c| matches!(c.data.borrow().value, NodeValue::List(_)))
        .flat_map(|c| c.children())
        .filter(|c| {
            matches!(
                c.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            )
        });

    for list_item_node in iter_list_items {
        let mut children = list_item_node.children();

        let Some(para) = children.next() else {
            continue;
        };

        if !matches!(para.data.borrow().value, NodeValue::Paragraph) {
            continue;
        }

        let Ok(link) = extract_link_from_paragraph(para) else {
            continue;
        };

        let link = links.entry(link.url.clone()).or_insert_with(|| link);

        for child in children {
            if !matches!(child.data.borrow().value, NodeValue::List(_)) {
                continue;
            }

            if extract_metadata_from_child_list(link, child).is_err() {
                continue;
            }
        }
    }

    for link in links.into_values() {
        let link = enrichment::enrich_link(link, store, &link_source).await?;
        if let Err(e) = store.write(&link).await {
            eprintln!("error fetching {}: {:?}", link.url, e);
        }
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

        match first_child_text.split(':').next() {
            Some("tags") => {
                let mut tags: HashSet<_> = first_child_text["tags:".len()..]
                    .trim()
                    .split(',')
                    .filter(|xs| !xs.is_empty())
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
                                    .split(',')
                                    .map(|xs| xs.trim().to_string()),
                            );
                        }
                    }
                }

                link.tags.extend(tags);
            }

            Some("via") => {
                link.via = Some(parse_via(first_child_text["via:".len()..].trim()));
            }

            Some("notes") => {
                if let Some(child) = list_item_children.next() {
                    if !matches!(child.data.borrow().value, NodeValue::List(_)) {
                        continue;
                    }

                    let notes = itertools::join(
                        itertools::chain(
                            link.notes.iter().map(|xs| xs.to_string()),
                            child
                                .children()
                                .flat_map(|list_item| list_item.children())
                                .filter_map(|node| fmt_cmark(node).ok()),
                        ),
                        "\n",
                    )
                    .trim()
                    .to_string();

                    link.notes = if notes.is_empty() { None } else { Some(notes) };
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn parse_via(text: &str) -> Via {
    if text.starts_with('@') {
        return Via::Friend(text.trim().to_string());
    }

    match text.split(':').next() {
        Some("http") | Some("https") => Via::Link(text.trim().to_string()),
        _ => Via::Freeform(text.to_string()),
    }
}

fn extract_link_from_paragraph<'a>(graf: &'a Node<'a, RefCell<Ast>>) -> eyre::Result<Link> {
    // clippy is wrong, here! if we don't find what we're looking for on an element, we `continue`!
    #[allow(clippy::never_loop)]
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

        let title = title.trim();

        return Ok(Link {
            url: url.to_string(),
            title: if title.is_empty() {
                let anchor_children: Result<String, _> = child.children().map(fmt_cmark).collect();
                if let Ok(text) = anchor_children {
                    Some(text)
                } else {
                    None
                }
            } else {
                Some(title.to_string())
            },
            ..Default::default()
        });
    }

    let content = fmt_cmark(graf)?;

    let text = content.strip_prefix("\\[ \\]").unwrap_or(content.as_str());

    let mut indent = 0;

    for piece in text.split(&['-', ':', ' '][..]) {
        match piece {
            "https" | "http" => {
                let title = text[0..indent]
                    .trim_start_matches(['-', ':', ' ', '\t'])
                    .trim_end_matches(&['-', ':', ' ', '\t']);

                let mut url_bits = text[indent..].split_whitespace();

                let Some(url) = url_bits.next() else { continue };

                let title = if title.is_empty() {
                    // this handles the case where SOME reckless person wrote their
                    // links like "https://url.great (but hey here is the title lol sorry)"
                    text[indent..].trim()[url.len()..].to_string()
                } else {
                    title.trim().to_string()
                };

                let title = if title.is_empty() { None } else { Some(title) };

                let Ok(mut url) = url.replace('\\', "").parse::<url::Url>() else {
                    return Err(eyre::eyre!("empty paragraph, no link"))
                };

                url.set_fragment(None);
                let url = url.to_string();

                return Ok(Link {
                    title,
                    url,
                    ..Default::default()
                });
            }

            t => {
                indent += piece.len() + 1;
            }
        }
    }

    Err(eyre::eyre!("empty paragraph, no link"))
}

fn fmt_cmark<'a>(node: &'a Node<'a, RefCell<Ast>>) -> eyre::Result<String> {
    let mut output = Vec::with_capacity(512);
    comrak::format_commonmark(node, &ComrakOptions::default(), &mut output)?;

    if output.is_empty() {
        Ok(Default::default())
    } else {
        output.pop();
        String::from_utf8(output).map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::ReadLinkInformation;
    use super::*;
    use futures::StreamExt;
    use sqlx::pool::PoolOptions;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::Sqlite;

    #[sqlx::test]
    async fn test_parse_tags(
        pool_opts: PoolOptions<Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> eyre::Result<()> {
        let store =
            crate::DummyWrap::new(SqliteStore::with_connection_options(connect_opts).await?);

        process_input(
            r#"
- plain text link title: https://a.com/
    - tags: hello, there, gawrsh, this is great, yep, ok
- [markdown style](https://b.com/)
    - tags:
        - hello
        - there
        - gawrsh
        - this is great
        - yep, ok
"#,
            &store,
        )
        .await?;

        let link_a = store.get("https://a.com/").await?;

        dbg!(link_a);
        Ok(())
    }

    #[sqlx::test]
    async fn test_note_newlines(
        pool_opts: PoolOptions<Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> eyre::Result<()> {
        Ok(())
    }

    #[sqlx::test]
    async fn test_create_then_update_leaves_enriched_metadata_in_place(
        pool_opts: PoolOptions<Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> eyre::Result<()> {
        let store =
            crate::DummyWrap::new(SqliteStore::with_connection_options(connect_opts).await?);
        // Make sure that:
        // - title
        // - image
        // - TKTK
        //
        // are preserved when updating a previously-created link
        Ok(())
    }

    #[tokio::test]
    async fn it_works() -> eyre::Result<()> {
        let store = super::HttpClientWrap::wrap(
            SqliteStore::with_connection_string("sqlite::memory:").await?,
        );

        process_input(
            r#"
# read links

- read link *markdown*: https://foo.baz
    - via: @garybusey
    - tags: alpha, beta
        - gamma
        - epsilon, mu
    - notes:
        - # just testing
        - wow, so interesting
        - uhh
          - Ok(result)
          - and yet what now
- read link *other markdown*: https://foo.baz
    - tags: foo, bar, baz
    - via: https://bar.dev/baz
    - notes:
        - testing
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
            let Ok(frontmatter): Result<Frontmatter, _> = v.try_into() else { continue };
            let toml_out = toml::to_string_pretty(&frontmatter)?;

            eprintln!("+++\n{}\n+++\n{}", toml_out, frontmatter.notes());
        }

        Ok(())
    }
}
