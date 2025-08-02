use anyhow::Result;
use crossbeam::queue::SegQueue;
use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use sled;
use tokio::{io::BufReader, net::UnixStream};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum SledTree {
    Fallback,
    Default,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GlobalSummary {
    pub default: Summary,
    pub fallback: Summary,
}

impl Default for GlobalSummary {
    fn default() -> Self {
        Self {
            default: Summary::new(),
            fallback: Summary::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Summary {
    #[serde(rename = "totalRequests")]
    pub total_requests: u64,
    #[serde(rename = "totalAmount")]
    pub total_amount: f64,
}

impl Summary {
    pub fn new() -> Self {
        Summary {
            total_requests: 0,
            total_amount: 0.0,
        }
    }
}

impl FromIterator<sled::Result<(sled::IVec, sled::IVec)>> for Summary {
    fn from_iter<I: IntoIterator<Item = sled::Result<(sled::IVec, sled::IVec)>>>(iter: I) -> Self {
        iter.into_iter()
            .filter_map(Result::ok)
            .map(|(_, value)| {
                f64::from_be_bytes(value.as_ref().try_into().expect("Expected 8 bytes"))
            })
            .fold(Summary::new(), |mut summary, amount| {
                summary.total_amount += amount;
                summary.total_requests += 1;
                summary
            })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PaymentDTO {
    #[serde(rename = "correlationId")]
    pub correlation_id: Uuid,
    pub amount: f64,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DBWrite {
    pub key: String,
    pub value: f64,
    pub tree: SledTree,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DBRead {
    pub from: String,
    pub to: String,
}

#[derive(Clone)]
pub struct UnixConnectionPool {
    connections: Arc<SegQueue<UnixStream>>,
    pool_size: usize,
    current_size: Arc<AtomicUsize>,
    path: PathBuf,
}

impl UnixConnectionPool {
    /// Create a new connection pool with the specified path and pool size
    pub async fn new<P: AsRef<Path>>(path: P, pool_size: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let pool = Self {
            connections: Arc::new(SegQueue::new()),
            pool_size,
            current_size: Arc::new(AtomicUsize::new(0)),
            path,
        };

        // Pre-populate the pool
        pool.populate_pool().await?;
        Ok(pool)
    }

    /// Create a new connection pool with lazy initialization
    pub fn new_lazy<P: AsRef<Path>>(path: P, pool_size: usize) -> Self {
        Self {
            connections: Arc::new(SegQueue::new()),
            pool_size,
            current_size: Arc::new(AtomicUsize::new(0)),
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Pre-populate the pool with connections (best effort)
    async fn populate_pool(&self) -> Result<()> {
        let mut errors = Vec::new();
        let mut success_count = 0;

        for i in 0..self.pool_size {
            match self.create_connection().await {
                Ok(conn) => {
                    self.connections.push(conn);
                    self.current_size.fetch_add(1, Ordering::Relaxed);
                    success_count += 1;
                }
                Err(e) => {
                    errors.push(format!("Connection {}: {}", i, e));
                }
            }
        }

        if success_count == 0 && !errors.is_empty() {
            anyhow::bail!("Failed to create any connections: {:?}", errors);
        }

        if !errors.is_empty() {
            eprintln!(
                "Warning: Some connections failed to initialize: {:?}",
                errors
            );
        }

        Ok(())
    }

    /// Create a new connection to the Unix socket
    async fn create_connection(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.path).await.map_err(Into::into)
    }

    /// Get a connection from the pool (non-blocking, lockfree)
    pub fn try_get_connection(&self) -> Option<UnixStream> {
        match self.connections.pop() {
            Some(conn) => {
                self.current_size.fetch_sub(1, Ordering::Relaxed);
                Some(conn)
            }
            None => None,
        }
    }

    /// Get a connection from the pool or create a new one
    pub async fn acquire(&self) -> Result<PooledConnection> {
        // First try to get from pool (lockfree)
        if let Some(conn) = self.try_get_connection() {
            return Ok(PooledConnection::new(conn, self.clone()));
        }

        // Pool is empty, create new connection
        let conn = self.create_connection().await?;
        Ok(PooledConnection::new(conn, self.clone()))
    }

    /// Return a connection to the pool (lockfree)
    pub fn return_connection(&self, conn: UnixStream) {
        let current = self.current_size.load(Ordering::Relaxed);
        if current < self.pool_size {
            self.connections.push(conn);
            self.current_size.fetch_add(1, Ordering::Relaxed);
        }
        // If pool is full, connection is dropped
    }

    /// Close all connections in the pool (best effort)
    pub fn close(&self) {
        while self.connections.pop().is_some() {
            self.current_size.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Get the pool size
    pub fn pool_size(&self) -> usize {
        self.pool_size
    }

    /// Check if pool is approximately empty
    pub fn is_empty(&self) -> bool {
        self.current_size.load(Ordering::Relaxed) == 0
    }
}

/// A connection wrapper that automatically returns the connection to the pool when dropped
pub struct PooledConnection {
    conn: Option<UnixStream>,
    pool: UnixConnectionPool,
}

impl PooledConnection {
    fn new(conn: UnixStream, pool: UnixConnectionPool) -> Self {
        Self {
            conn: Some(conn),
            pool,
        }
    }

    /// Get a reference to the underlying connection
    pub fn as_ref(&self) -> Option<&UnixStream> {
        self.conn.as_ref()
    }

    /// Get a mutable reference to the underlying connection
    pub fn as_mut(&mut self) -> Option<&mut UnixStream> {
        self.conn.as_mut()
    }

    /// Take ownership of the connection (prevents automatic return to pool)
    pub fn take(mut self) -> Option<UnixStream> {
        self.conn.take()
    }

    /// Check if connection is still valid (not taken)
    pub fn is_valid(&self) -> bool {
        self.conn.is_some()
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.return_connection(conn);
        }
    }
}

impl std::ops::Deref for PooledConnection {
    type Target = UnixStream;

    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().expect("Connection was taken")
    }
}

impl std::ops::DerefMut for PooledConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().expect("Connection was taken")
    }
}
