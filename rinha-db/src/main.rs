use shared_types::{GlobalSummary, Message, SledTree, Summary};
use sled::{self, Db, Tree};
use std::env;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task;

// Commands that will be processed sequentially
#[derive(Debug)]
enum Command {
    Read {
        from: String,
        to: String,
        response_tx: mpsc::UnboundedSender<String>,
    },
    Write {
        key: String,
        value: f64,
        tree: SledTree,
    },
    Purge,
}

async fn database_worker(
    mut command_rx: mpsc::UnboundedReceiver<Command>,
    default_tree: Tree,
    fallback_tree: Tree,
    db: Db,
) {
    while let Some(command) = command_rx.recv().await {
        match command {
            Command::Read {
                from,
                to,
                response_tx,
            } => {
                let result = (|| -> anyhow::Result<String> {
                    let default =
                        Summary::from_iter(default_tree.range(from.as_str()..=to.as_str()));
                    let fallback =
                        Summary::from_iter(fallback_tree.range(from.as_str()..=to.as_str()));
                    let global_summary = GlobalSummary { default, fallback };
                    serde_json::to_string(&global_summary).map_err(|e| e.into())
                })();

                match result {
                    Ok(response) => {
                        let _ = response_tx.send(response);
                    }
                    Err(e) => {
                        eprintln!("Database error during read: {}", e);
                    }
                }
            }
            Command::Write { key, value, tree } => {
                let result = (|| -> anyhow::Result<()> {
                    let bytes = value.to_be_bytes();
                    match tree {
                        SledTree::Fallback => {
                            fallback_tree.insert(key.as_bytes(), &bytes)?;
                        }
                        SledTree::Default => {
                            default_tree.insert(key.as_bytes(), &bytes)?;
                        }
                    }
                    Ok(())
                })();

                if let Err(e) = result {
                    eprintln!("Database error during write: {}", e);
                }
            }
            Command::Purge => {
                let result = (|| -> anyhow::Result<()> {
                    db.drop_tree("fallback")?;
                    db.drop_tree("default")?;
                    Ok(())
                })();

                if let Err(e) = result {
                    eprintln!("Database error during purge: {}", e);
                }
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    command_tx: mpsc::UnboundedSender<Command>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).lines();

    // Create a response channel for this client's reads
    let (response_tx, mut response_rx) = mpsc::unbounded_channel::<String>();

    // Spawn a task to handle responses for this client
    let writer_task = {
        task::spawn(async move {
            while let Some(response) = response_rx.recv().await {
                if writer.write_all(response.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        })
    };

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<Message>(&line) {
            Ok(message) => match message {
                Message::Read { from, to } => {
                    if command_tx
                        .send(Command::Read {
                            from,
                            to,
                            response_tx: response_tx.clone(),
                        })
                        .is_err()
                    {
                        eprintln!("Database worker has shut down");
                        break;
                    }
                }
                Message::Write { key, value, tree } => {
                    if command_tx
                        .send(Command::Write { key, value, tree })
                        .is_err()
                    {
                        eprintln!("Database worker has shut down");
                        break;
                    }
                }
                Message::Purge => {
                    if command_tx.send(Command::Purge).is_err() {
                        eprintln!("Database worker has shut down");
                        break;
                    }
                }
            },
            Err(e) => {
                eprintln!("Failed to parse message: {}", e);
            }
        }
    }

    // Clean up the writer task
    writer_task.abort();
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let socket_path = env::var("RINHADB_SOCK").unwrap_or("/tmp/rinha.sock".to_string());
    if Path::new(socket_path.as_str()).exists() {
        std::fs::remove_file(socket_path.clone())?;
    }

    let database_url: String = "app_db".to_string();
    let db = sled::open(database_url)?;
    let default_tree = db.open_tree("default")?;
    let fallback_tree = db.open_tree("fallback")?;

    // Create a channel for database commands
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    // Spawn the database worker that will process all operations sequentially
    let db_clone = db.clone();
    let default_tree_clone = default_tree.clone();
    let fallback_tree_clone = fallback_tree.clone();
    task::spawn(async move {
        database_worker(
            command_rx,
            default_tree_clone,
            fallback_tree_clone,
            db_clone,
        )
        .await;
    });

    let listener = UnixListener::bind(socket_path.as_str())?;
    println!("RinhaDB listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        let command_tx = command_tx.clone();

        task::spawn(async move {
            if let Err(e) = handle_client(stream, command_tx).await {
                eprintln!("Error handling client: {}", e);
            }
        });
    }
}
