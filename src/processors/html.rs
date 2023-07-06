use crate::{processors::LinkReadProcessor, Link, LinkReader, LinkWriter};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

use scraper::{Html, Selector};
use std::collections::HashMap;

const DEFAULT_LINEWRAP_AT: usize = 80;

/// An external store is used for data associated with the link
/// that we are unlikely to use when exporting static site data, especially
/// when that data is large or requires computation. This includes the original source data and text
/// extractions.
pub struct HtmlProcessorWrap<T> {
    inner: T,
}

impl<T> HtmlProcessorWrap<T> {
    pub fn wrap(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl<T> LinkReadProcessor for HtmlProcessorWrap<T>
where
    T: Send + Sync + LinkReader,
{
    type Inner = T;

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }
}

#[async_trait::async_trait]
impl<T: LinkWriter + Send + Sync> LinkWriter for HtmlProcessorWrap<T> {
    async fn write(&self, mut link: Link) -> eyre::Result<bool> {
        // TODO: this is where encodings _would_ go. See encoding_rs, windows-1252 for latin1
        if link.last_processed().is_none() && link.src().is_some() && link.is_html() {
            link.last_processed = Some(Utc::now());
            link = process_html(link)?;
            link.extracted_text = Some(html2text::from_read(
                link.src().unwrap_or(b""),
                DEFAULT_LINEWRAP_AT,
            ));
        }
        self.inner.write(link).await
    }
}

fn process_html(mut link: Link) -> eyre::Result<Link> {
    let html = String::from_utf8_lossy(link.src().unwrap_or(b""));
    let doc = Html::parse_document(html.as_ref());
    let mut pubdate: Option<(usize, DateTime<Utc>)> = None;
    let mut title: Option<(usize, String)> = None;
    let mut image: Option<(usize, String)> = None;
    let mut meta = HashMap::new();

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

    let selector = Selector::parse(
        r#"
        head title,head meta,time
    "#,
    )
    .expect("selector failed to parse");

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
                let mut content = element.text().next();

                for (attrname, attrvalue) in ev.attrs() {
                    match attrname {
                        "name" => name.replace(attrvalue),

                        // RDFa
                        "property" => name.replace(attrvalue),

                        // Microdata
                        "itemprop" => name.replace(attrvalue),

                        "content" => content.replace(attrvalue),
                        _ => continue,
                    };

                    if let (Some(ref name), Some(ref content)) = (&name, &content) {
                        meta.entry(name.to_string())
                            .or_insert_with(Vec::new)
                            .push(content.to_string());
                    }

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

                        (Some(_), _) => continue,
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

    link.title = link.title.or_else(|| title.map(|(_, xs)| xs));
    link.published_at = link.published_at.or_else(|| pubdate.map(|(_, xs)| xs));
    link.image = link.image.or_else(|| image.map(|(_, xs)| xs));
    link.meta = link.meta.or(Some(meta));
    Ok(link)
}
