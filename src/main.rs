use futures::{future::join_all, StreamExt};

use std::path::PathBuf;

use clap::Parser;
use likelike::{
    process_input, Frontmatter, HttpClientWrap, LinkSource, ReadLinkInformation, SqliteStore,
};

/// Process markdown-formatted linkdump files and store them in a sqlite database.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// If not given, defaults to the local data dir per the "dirs" crate. E.g., on macOS, this
    /// will be "sqlite:///Users/foo/Library/Application Support/likelike.sqlite3".
    #[arg(short, long)]
    database_url: Option<String>,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Import links from a set of files. Files are parsed for nested markdown lists containing
    /// links in markdown-anchor or "text: url" format.
    ///
    /// Sublists are used to add metadata.
    ///
    /// E.g.:
    ///
    /// ```
    /// - some link: https://foo.bar/baz
    ///   - notes:
    ///     - # heading
    ///     - some more text
    ///   - tags:
    ///     - a
    ///     - b
    /// ```
    ///
    Import {
        files: Vec<PathBuf>,

        /// Pass this argument to display imported link data.
        #[arg(long)]
        display_links: bool,
    },

    /// Export links from the database as zola markdown documents with Link metadata included in
    /// frontmatter.
    Export { output: PathBuf },

    /// Show information about a given link. Accepts globstar patterns (be sure to single-quote
    /// them!)
    Show {
        url: String,

        /// Only show metadata information: meta tags & http headers
        #[arg(short, long)] meta: bool
    },
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Args::parse();

    let store = if let Some(db_url) = cli.database_url {
        SqliteStore::with_connection_string(db_url).await?
    } else {
        SqliteStore::new().await
    };

    match cli.command {
        Commands::Show { url, meta } => {
            let mut links = store.glob(url.as_str()).await?;

            while let Some(link) = links.next().await {
                if meta {
                    let link_meta = serde_json::to_string_pretty(&link.meta()).unwrap_or_default();
                    let link_headers = serde_json::to_string_pretty(&link.http_headers()).unwrap_or_default();
                    eprintln!("{}", link_meta);
                    eprintln!("{}", link_headers);
                } else if let Some(src) = link.extract_text() {
                    eprintln!("{}", src);
                } else {
                    eprintln!("{:?}", link);
                }
            }
        }

        Commands::Export { output } => {
            let mut links = store.values().await?;

            while let Some(link) = links.next().await {
                if link.read_at().is_none() {
                    continue;
                }

                let Ok(frontmatter): Result<Frontmatter, _> = link.try_into() else { continue };
                let mut path = output.clone();
                path.push(frontmatter.filename());
                std::fs::write(
                    path,
                    format!(
                        "+++\n{}\n+++\n{}",
                        toml::to_string_pretty(&frontmatter)?,
                        frontmatter.notes()
                    ),
                )?;
            }
        }

        Commands::Import {
            files,
            display_links,
        } => {
            let store = HttpClientWrap::wrap(store);
            let store = &store;

            let mut resolved_files = Vec::new();
            _find_markdown_files(&mut resolved_files, files, FindMode::Explicit)?;

            let mut futs = Vec::with_capacity(resolved_files.len());
            for file in resolved_files.into_iter() {
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

            if display_links {
                let mut links = store.values().await?;

                while let Some(link) = links.next().await {
                    eprintln!("{:?}", link);
                }
            }
        }
    }

    Ok(())
}

#[derive(PartialEq)]
enum FindMode {
    Explicit,
    Implicit,
}

fn _find_markdown_files(
    output: &mut Vec<PathBuf>,
    files: Vec<PathBuf>,
    mode: FindMode,
) -> eyre::Result<()> {
    output.reserve(files.len());
    for file in files.into_iter() {
        let Ok(metadata) = std::fs::metadata(file.as_path()) else { continue };
        if metadata.is_dir() {
            let entries: Vec<_> = std::fs::read_dir(file.as_path())?
                .filter_map(|file| Some(file.ok()?.path()))
                .collect();

            _find_markdown_files(output, entries, FindMode::Implicit)?;
        } else if mode == FindMode::Implicit {
            // filter "implicit" files by extension.
            let Some(ext) = file.extension() else { continue };

            if "md" == ext {
                output.push(file);
            }
        } else {
            output.push(file);
        }
    }

    Ok(())
}
