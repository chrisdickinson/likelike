use futures::future::join_all;
use std::path::PathBuf;

use clap::Parser;
use likelike::{process_input, HttpClientWrap, LinkSource, SqliteStore};

/// Process markdown-formatted linkdump files and store them in a sqlite database. The database
/// defaults to in-memory.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long)]
    database_url: Option<String>,
}

#[derive(Parser, Debug)]
enum Commands {
    Import { files: Vec<PathBuf> },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Args::parse();

    match cli.command {
        Commands::Import { files } => {
            let store = if let Some(db_url) = cli.database_url {
                SqliteStore::with_connection_string(db_url).await?
            } else {
                SqliteStore::new().await
            };

            let store = HttpClientWrap::wrap(store);
            let store = &store;
            let mut futs = Vec::with_capacity(files.len());
            for file in files.into_iter() {
                futs.push(async move {
                    let link_source = LinkSource::from_path(file.as_path())?;
                    process_input(link_source, store).await?;
                    Ok(file) as eyre::Result<PathBuf>
                });
            }

            for result in join_all(futs.into_iter()).await {
                let Ok(file) = result else { continue };
                eprintln!("processed \"{}\"", file.to_string_lossy());
            }
        }
    }

    Ok(())
}
