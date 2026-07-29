//! Cross-platform IPC transport. On Linux this is a Unix domain socket;
//! on Windows it's a named pipe. We expose a unified async read/write API
//! over [`tokio::io::AsyncRead`] / [`tokio::io::AsyncWrite`].

use crate::{IpcMessage, IpcRequest, IpcResponse};
use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub trait IpcRead: AsyncRead + Unpin + Send {}
impl<T: AsyncRead + Unpin + Send> IpcRead for T {}
pub trait IpcWrite: AsyncWrite + Unpin + Send {}
impl<T: AsyncWrite + Unpin + Send> IpcWrite for T {}

pub fn default_socket_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(r"\\.\pipe\inputsync"))
    }
    #[cfg(unix)]
    {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Ok(runtime.join("inputsync.sock"))
    }
}

// -------- Listener --------

#[cfg(unix)]
pub struct IpcListener {
    listener: tokio::net::UnixListener,
}

#[cfg(unix)]
pub fn listen(path: &std::path::Path) -> Result<IpcListener> {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener =
        tokio::net::UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    Ok(IpcListener { listener })
}

#[cfg(unix)]
impl IpcListener {
    pub async fn accept(&self) -> Result<IpcConnection> {
        let (stream, _) = self.listener.accept().await?;
        let (r, w) = stream.into_split();
        Ok(IpcConnection {
            reader: BufReader::new(Box::new(r)),
            writer: Box::new(w),
        })
    }
}

#[cfg(windows)]
pub struct IpcListener {
    path: String,
}

#[cfg(windows)]
pub fn listen(path: &std::path::Path) -> Result<IpcListener> {
    Ok(IpcListener {
        path: path.to_string_lossy().into_owned(),
    })
}

#[cfg(windows)]
impl IpcListener {
    pub async fn accept(&self) -> Result<IpcConnection> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(&self.path)
            .with_context(|| format!("create pipe {}", self.path))?;
        server.connect().await.context("await pipe client")?;
        // Split via tokio::io::split since named_pipe::NamedPipeServer
        // implements AsyncRead + AsyncWrite but not `into_split`.
        let (r, w) = tokio::io::split(server);
        Ok(IpcConnection {
            reader: BufReader::new(Box::new(r)),
            writer: Box::new(w),
        })
    }
}

pub struct IpcConnection {
    reader: BufReader<Box<dyn IpcRead>>,
    writer: Box<dyn IpcWrite>,
}

impl IpcConnection {
    pub async fn write_request(&mut self, req: &IpcRequest) -> Result<()> {
        write_json_line(&mut self.writer, req).await
    }

    pub async fn write_response(&mut self, resp: &IpcResponse) -> Result<()> {
        write_json_line(&mut self.writer, resp).await
    }

    pub async fn write_event(&mut self, evt: &crate::IpcEvent) -> Result<()> {
        write_json_line(&mut self.writer, evt).await
    }

    pub async fn read_request(&mut self) -> Result<IpcRequest> {
        read_json_line(&mut self.reader).await
    }

    pub async fn read_message(&mut self) -> Result<IpcMessage> {
        read_json_line(&mut self.reader).await
    }
}

async fn write_json_line<T: Serialize, W: IpcWrite>(writer: &mut W, val: &T) -> Result<()> {
    let mut line = serde_json::to_vec(val).map_err(|e| anyhow!("serialize ipc message: {e}"))?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_json_line<T: DeserializeOwned, R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<T> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Err(anyhow!("ipc peer disconnected"));
    }
    let trimmed = line.trim_end();
    serde_json::from_str(trimmed).map_err(|e| anyhow!("parse ipc message: {e}: {trimmed}"))
}

// -------- Client --------

pub struct IpcClient {
    conn: IpcConnection,
}

impl IpcClient {
    pub async fn connect(path: &std::path::Path) -> Result<Self> {
        #[cfg(unix)]
        {
            let stream = tokio::net::UnixStream::connect(path).await?;
            let (r, w) = stream.into_split();
            Ok(Self {
                conn: IpcConnection {
                    reader: BufReader::new(Box::new(r)),
                    writer: Box::new(w),
                },
            })
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            let pipe = ClientOptions::new()
                .open(path.to_string_lossy().as_ref())
                .with_context(|| format!("connect {}", path.display()))?;
            let (r, w) = tokio::io::split(pipe);
            Ok(Self {
                conn: IpcConnection {
                    reader: BufReader::new(Box::new(r)),
                    writer: Box::new(w),
                },
            })
        }
    }

    pub async fn request(&mut self, req: &IpcRequest) -> Result<IpcResponse> {
        self.conn.write_request(req).await?;
        match self.conn.read_message().await? {
            IpcMessage::Response(r) => Ok(r),
            IpcMessage::Event(_) => Err(anyhow!("unexpected event before response")),
        }
    }

    pub async fn next_message(&mut self) -> Result<IpcMessage> {
        self.conn.read_message().await
    }
}
