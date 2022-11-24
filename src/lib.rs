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
use std::{cell::RefCell, collections::HashSet, pin::Pin};

mod domain;
mod stores;

pub use crate::domain::*;
pub use crate::stores::*;

/// Read link information from the link store.
#[async_trait::async_trait]
pub trait ReadLinkInformation {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>>;
    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>>;
}

/// Write link information back to the link store.
#[async_trait::async_trait]
pub trait WriteLinkInformation {
    async fn update(&self, link: &Link) -> eyre::Result<bool>;
    async fn create(&self, link: &Link) -> eyre::Result<bool>;
}

#[async_trait::async_trait]
pub trait FetchLinkMetadata {
    type Headers: TryInto<HeaderMap>;
    type Body: Stream<Item = bytes::Bytes>;

    async fn fetch(&self, link: &Link) -> eyre::Result<Option<(Self::Headers, Self::Body)>>;
}

async fn insert_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<bool>
where
    Store: FetchLinkMetadata + ReadLinkInformation + WriteLinkInformation + Send + Sync,
{
    if let Some(known_link) = store.get(link.url.as_str()).await? {
        link.read_at = known_link.read_at.or(link_source.created);
        link.found_at = known_link.found_at.or(link_source.created);
        link.from_filename = known_link
            .from_filename
            .or_else(|| link_source.filename_string());

        store.update(&link).await
    } else {
        link.found_at = link_source.created;
        link.from_filename = link_source.filename_string();

        if link.notes.is_some() {
            link.read_at = link_source.created;
        }

        if let Some((headers, body)) = store.fetch(&link).await? {
            if let Ok(headers) = headers.try_into() {
                eprintln!("{} => {:?}", link.url, headers);
            }
        }

        store.create(&link).await
    }
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
        insert_link(link, store, &link_source).await?;
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

    let content = fmt_cmark(graf)?;
    let text = if content.starts_with("\\[ \\]") {
        &content[5..]
    } else {
        content.as_str()
    };

    let mut indent = 0;

    for piece in text.split(&['-', ':', ' '][..]) {
        match piece {
            "https" | "http" => {
                let title = text[0..indent]
                    .trim_start_matches(&['-', ':', ' ', '\t'])
                    .trim_end_matches(&['-', ':', ' ', '\t']);

                let mut url_bits = text[indent..].trim().split_whitespace();

                let Some(url) = url_bits.next() else { continue };

                let title = if title.is_empty() {
                    // this handles the case where SOME reckless person wrote their
                    // links like "https://url.great (but hey here is the title lol sorry)"
                    text[indent..].trim()[url.len()..].to_string()
                } else {
                    title.to_string()
                };

                return Ok(Link {
                    title: title.to_string(),
                    url: url.replace('\\', ""),
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
    String::from_utf8(output).map_err(|e| e.into())
}

#[cfg(test)]
mod tests {
    use super::ReadLinkInformation;
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn it_works() -> eyre::Result<()> {
        let store = super::HttpClientWrap::wrap(SqliteStore::new().await);

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
