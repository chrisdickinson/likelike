mod external;
mod memory;
mod sqlite;

use std::pin::Pin;

pub use external::*;
use futures::Stream;
pub use memory::*;
pub use sqlite::*;

use crate::Link;

/// Read link information from the link store.
#[async_trait::async_trait]
pub trait LinkReader {
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>>;
    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a + Send>>>;
    async fn glob<'a, 'b: 'a>(
        &'a self,
        pattern: &'b str,
    ) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
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
pub trait LinkWriter {
    async fn write(&self, link: Link) -> eyre::Result<bool>;
}
