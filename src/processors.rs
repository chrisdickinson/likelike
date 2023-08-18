use crate::{Link, LinkReader};
use futures::Stream;
use std::pin::Pin;
mod html;
mod http;
mod pdf;
mod txt;

pub use html::*;
pub use http::*;
pub use txt::*;
pub use pdf::*;

#[async_trait::async_trait]
pub(crate) trait LinkReadProcessor {
    type Inner: Send + Sync + LinkReader;
    async fn hydrate(&self, link: Link) -> eyre::Result<Link> {
        Ok(link)
    }
    fn inner(&self) -> &Self::Inner;
}

#[async_trait::async_trait]
impl<T> LinkReader for T
where
    T: LinkReadProcessor + Send + Sync,
    T::Inner: LinkReader + Send + Sync,
{
    async fn get(&self, link: &str) -> eyre::Result<Option<Link>> {
        match self.inner().get(link).await {
            Ok(Some(link)) => self.hydrate(link).await.map(Option::Some),
            xs => xs,
        }
    }

    async fn values<'a>(&'a self) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let links = self.inner().values().await?;
        let links = async_stream::stream! {
            for await link in links {
                let Ok(link) = self.hydrate(link).await else { continue };
                yield link;
            }
        };

        Ok(Box::pin(links))
    }

    async fn glob<'a, 'b: 'a>(
        &'a self,
        pattern: &'b str,
    ) -> eyre::Result<Pin<Box<dyn Stream<Item = Link> + 'a>>> {
        let links = self.inner().glob(pattern).await?;
        let links = async_stream::stream! {
            for await link in links {
                let Ok(link) = self.hydrate(link).await else { continue };
                yield link;
            }
        };

        Ok(Box::pin(links))
    }
}
