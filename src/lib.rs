#![allow(dead_code)]
#![allow(unused_variables)]

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use comrak::{
    self,
    arena_tree::Node,
    nodes::{Ast, NodeLink, NodeValue},
    parse_document, Arena, ComrakOptions,
};
use futures::{future::join_all, pin_mut, Stream, StreamExt, TryFutureExt};
use html5ever::driver::{self, ParseOpts};
use reqwest::header::HeaderMap;
use scraper::{Html, Selector};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    pin::Pin,
    str::from_utf8,
};
use tendril::TendrilSink;

mod domain;
// mod html;
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

async fn enrich_link<Store>(
    mut link: Link,
    store: &Store,
    link_source: &LinkSource<'_>,
) -> eyre::Result<(Link, bool)>
where
    Store: FetchLinkMetadata + ReadLinkInformation + Send + Sync,
{
    let update = if let Some(known_link) = store.get(link.url.as_str()).await? {
        link.read_at = known_link.read_at.or_else(|| {
            if let Some(notes) = link.notes() {
                if !notes.trim().is_empty() {
                    return Some(Utc::now());
                }
            }

            None
        });

        link.found_at = known_link.found_at.or(link_source.created);
        link.from_filename = known_link
            .from_filename
            .or_else(|| link_source.filename_string());
        true
    } else {
        link.found_at = link_source.created;
        link.from_filename = link_source.filename_string();

        if let Some(notes) = link.notes() {
            if !notes.trim().is_empty() {
                link.read_at = link_source.created;
            }
        }

        if let Some((headers, body)) = store.fetch(&link).await? {
            let mut pubdate: Option<(usize, DateTime<Utc>)> = None;
            let mut title: Option<(usize, String)> = None;
            let mut image: Option<(usize, String)> = None;

            let mut update_pubdate = |weight, pd: &str| {
                let Ok(pd) = NaiveDate::parse_from_str(pd, "%Y-%m-%d") else { return };
                let Some(pd) = pd.and_hms_milli_opt(0, 0, 0, 0) else { return };
                let Some(pd) = Local.from_local_datetime(&pd).latest() else { return };
                let pd = DateTime::<Utc>::from(pd);

                if let Some((current, _)) = pubdate {
                    if current < weight {
                        pubdate.replace((weight, pd));
                    }
                } else {
                    pubdate.replace((weight, pd));
                }
            };

            let mut update_image = |weight, candidate: &str| {
                if let Some((current, _)) = image {
                    if current < weight {
                        image.replace((weight, candidate.to_string()));
                    }
                } else {
                    image.replace((weight, candidate.to_string()));
                }
            };

            let mut update_title = |weight, candidate: &str| {
                if let Some((current, _)) = title {
                    if current < weight {
                        title.replace((weight, candidate.to_string()));
                    }
                } else {
                    title.replace((weight, candidate.to_string()));
                }
            };

            'html: {
                let Ok(headers) = headers.try_into() else { break 'html };
                let Some(content_type) = headers.get("Content-Type") else { break 'html };
                let Ok(
                    "text/html" |
                    "text/html;charset=utf-8" |
                    "text/html;charset=UTF-8" |
                    "text/html; charset=utf-8" |
                    "text/html; charset=UTF-8"
                ) = content_type.to_str() else { break 'html };

                let selector = Selector::parse(
                    r#"
                        head title,head meta,time
                    "#,
                )
                .expect("selector failed to parse");
                let mut parser = driver::parse_document(Html::new_document(), ParseOpts::default());

                pin_mut!(body);
                while let Some(chunk) = body.next().await {
                    let Ok(chunk) = from_utf8(chunk.as_ref()) else { break };
                    parser.process(chunk.into());
                }

                let doc = parser.finish();

                for element in doc.select(&selector) {
                    let ev = element.value();
                    match ev.name() {
                        "title" => {
                            let text: String = element.text().collect();
                            update_title(2, text.as_str());
                        }

                        "time" => {
                            let text: String = element.text().collect();
                            if let Some(datetime) = element.value().attr("datetime") {
                                update_pubdate(2, datetime);
                            }
                        }

                        "meta" => {
                            let mut name = None;
                            let mut content = None;
                            for (attrname, attrvalue) in ev.attrs() {
                                match attrname {
                                    "name" => name.replace(attrvalue),
                                    "content" => content.replace(attrvalue),
                                    _ => continue,
                                };

                                match (name, content) {
                                    (None, _) => continue,

                                    (Some("title"), Some(title)) => {
                                        update_title(5, title);
                                    }

                                    (Some("og:title"), Some(title)) => {
                                        update_title(4, title);
                                    }

                                    (Some("twitter:title"), Some(title)) => {
                                        update_title(3, title);
                                    }

                                    (Some("twitter:text:title"), Some(title)) => {
                                        update_title(0, title);
                                    }

                                    (Some("og:image:url"), Some(image)) => {
                                        update_image(5, image);
                                    }

                                    (Some("og:image"), Some(image)) => {
                                        update_image(5, image);
                                    }

                                    (Some("twitter:image:src"), Some(image)) => {
                                        update_image(4, image);
                                    }

                                    (Some("twitter:image"), Some(image)) => {
                                        update_image(4, image);
                                    }

                                    (Some("date.created"), Some(pubdate)) => {
                                        update_pubdate(5, pubdate);
                                    }

                                    (Some("date"), Some(pubdate)) => {
                                        update_pubdate(4, pubdate);
                                    }

                                    (Some("article:published_time"), Some(pubdate)) => {
                                        update_pubdate(3, pubdate);
                                    }

                                    (Some("DC.Date"), Some(pubdate)) => {
                                        update_pubdate(0, pubdate);
                                    }

                                    (Some(_), _) => break,
                                }

                                break;
                            }
                        }

                        _ => {
                            let mut collected = 0;
                            let text: String = element
                                .text()
                                .take_while(|x| {
                                    collected += x.len();
                                    collected < 512
                                })
                                .collect();

                            if let Some(idx) = text.find("ublished") {
                                // eprintln!("div.<published> = {}", &text[idx..].trim());
                            }
                        }
                    }
                }
            }

            link.title = link.title.or_else(|| title.map(|(_, xs)| xs));
            link.published_at = link.published_at.or_else(|| pubdate.map(|(_, xs)| xs));
            link.image = link.image.or_else(|| image.map(|(_, xs)| xs));
        }
        false
    };

    Ok((link, update))
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
        .flat_map(|c| c.children())
        .filter(|c| {
            matches!(
                c.data.borrow().value,
                NodeValue::Item(_) | NodeValue::TaskItem(_)
            )
        })
        .filter_map(|list_item_node| {
            let mut children = list_item_node.children();

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

                    if extract_metadata_from_child_list(&mut link, child).is_err() {
                        continue;
                    }
                }

                Some(link)
            } else {
                None
            }
        })
        .fold(HashMap::new(), |mut acc, link| {
            if !acc.contains_key(&link.url) {
                acc.insert(link.url.clone(), link);
            }

            acc
        });

    let links = links
        .into_values()
        .map(|link| enrich_link(link, store, &link_source))
        .map(|fut| {
            fut.and_then(|(link, should_update)| async move {
                if should_update {
                    store.update(&link).await
                } else {
                    store.create(&link).await
                }
            })
        });

    join_all(links).await;

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
        match first_child_text.split(':').next() {
            Some("tags") => {
                let mut tags: HashSet<_> = first_child_text["tags:".len()..]
                    .trim()
                    .split(',')
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

                    let mut notes: String = child
                        .children()
                        .flat_map(|list_item| list_item.children())
                        .filter_map(|node| fmt_cmark(node).ok())
                        .collect();

                    if let Some(prior_notes) = &link.notes {
                        notes = prior_notes.to_string() + &notes;
                    }

                    let notes = notes.trim().to_string();

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
                None
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
- no anchors: https://grump.bump/fromp#bomp

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
