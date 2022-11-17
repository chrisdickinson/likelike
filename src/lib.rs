#![allow(dead_code)]
#![allow(unused_variables)]

use chrono::{DateTime, Utc};
use comrak::{
    self,
    arena_tree::Node,
    nodes::{Ast, NodeLink, NodeValue},
    parse_document, Arena, ComrakOptions,
};
use std::{cell::RefCell, collections::HashSet};

trait ReadLinkInformation {
    fn get();
}

trait WriteLinkInformation {
    fn insert();
    fn update();
}

#[derive(Default, Clone, Debug)]
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

pub fn process_input(input: impl AsRef<str>) -> eyre::Result<()> {
    let arena = Arena::new();
    let opts = ComrakOptions::default();

    let root = parse_document(&arena, input.as_ref(), &opts);

    let mut links = Vec::new();

    for c in root.children() {
        if let NodeValue::List(_) = c.data.borrow().value {
            handle_list(&mut links, c)
        }
    }

    Ok(())
}

fn fmt_cmark<'a>(node: &'a Node<'a, RefCell<Ast>>) -> eyre::Result<String> {
    let mut output = Vec::with_capacity(512);
    comrak::format_commonmark(node, &ComrakOptions::default(), &mut output)?;
    String::from_utf8(output).map_err(|e| e.into())
}

fn handle_list<'a>(_links: &mut Vec<Link>, list: &'a Node<'a, RefCell<Ast>>) {
    for list_item_node in list.children() {
        if !matches!(
            list_item_node.data.borrow().value,
            NodeValue::Item(_) | NodeValue::TaskItem(_)
        ) {
            continue;
        }

        // extract the url
        let mut children = list_item_node.children().into_iter();

        if let Some(para) = children.next() {
            if !matches!(para.data.borrow().value, NodeValue::Paragraph) {
                continue;
            }

            let Ok(mut link) = extract_link_from_paragraph(para) else {
                continue;
            };

            for child in children {
                if !matches!(child.data.borrow().value, NodeValue::List(_)) {
                    continue;
                }

                if let Err(_) = extract_metadata_from_child_list(&mut link, child) {
                    continue;
                }
            }
        }
    }
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
            url: text,
            ..Default::default()
        },

        Some(text) => Link {
            title: text.to_string(),
            url: text[text.len()..].trim().to_string(),
            ..Default::default()
        },

        None => return Err(eyre::eyre!("empty paragraph, no link")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() -> eyre::Result<()> {
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
        )?;
        Ok(())
    }
}
