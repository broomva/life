//! PTY session handle — spawns processes in a pseudo-terminal.
//!
//! Uses `portable-pty 0.9` which provides `Child: Send + Sync` and
//! `MasterPty: Send`, enabling safe cross-thread usage.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use life_relay_core::RelayError;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem as _};
use tokio::sync::mpsc;
use uuid::Uuid;

/// A running PTY-backed process session.
///
/// Background tasks handle I/O:
/// - Reader task: drains PTY output → internal `output_rx` channel
/// - Writer task: feeds `input_tx` → PTY stdin
///
/// The master PTY is kept alive in `master` — dropping it would send SIGHUP.
pub struct PtyHandle {
    pub id: Uuid,
    pub pid: Option<u32>,
    input_tx: mpsc::Sender<String>,
    killer: Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    output_rx: Option<mpsc::Receiver<String>>,
    /// Kept alive to prevent SIGHUP being sent to the child prematurely.
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
}

impl std::fmt::Debug for PtyHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyHandle")
            .field("id", &self.id)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl PtyHandle {
    /// Spawn a new PTY session with the given command.
    ///
    /// Starts two background tasks — one reading PTY output into `output_rx`,
    /// one writing from `input_tx` into PTY stdin.
    pub fn spawn(id: Uuid, cmd: &[String], workdir: &str) -> Result<Self, RelayError> {
        if cmd.is_empty() {
            return Err(RelayError::SpawnFailed("empty command".to_string()));
        }

        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 220,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RelayError::SpawnFailed(e.to_string()))?;

        let mut builder = CommandBuilder::new(&cmd[0]);
        for arg in cmd.iter().skip(1) {
            builder.arg(arg);
        }
        builder.cwd(workdir);

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| RelayError::SpawnFailed(e.to_string()))?;

        let pid = child.process_id();
        let killer = child.clone_killer();

        // Drop the child handle — lifecycle managed via killer (SIGKILL) and
        // master PTY (SIGHUP on drop). Zombie reaping happens on relayd exit.
        drop(child);

        // Claim reader/writer before moving master into the keepalive Arc.
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| RelayError::SpawnFailed(e.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| RelayError::SpawnFailed(e.to_string()))?;

        let (output_tx, output_rx) = mpsc::channel::<String>(256);
        let (input_tx, mut input_rx) = mpsc::channel::<String>(64);

        // Background: drain PTY output → output channel.
        let session_id = id;
        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = String::from_utf8_lossy(&buf[..n]).into_owned();
                        if output_tx.blocking_send(data).is_err() {
                            break;
                        }
                    }
                }
            }
            tracing::debug!(session_id = %session_id, "PTY reader closed");
        });

        // Background: flush input channel → PTY stdin.
        tokio::task::spawn_blocking(move || {
            while let Some(data) = input_rx.blocking_recv() {
                if writer.write_all(data.as_bytes()).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            id,
            pid,
            input_tx,
            killer: Arc::new(Mutex::new(killer)),
            output_rx: Some(output_rx),
            master: Arc::new(Mutex::new(pair.master)),
        })
    }

    /// Resize the PTY window.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), RelayError> {
        self.master
            .lock()
            .map_err(|_| RelayError::Adapter("master lock poisoned".into()))?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RelayError::Adapter(e.to_string()))
    }

    /// Write raw input bytes to the PTY stdin.
    pub async fn send_input(&self, data: &str) -> Result<(), RelayError> {
        self.input_tx
            .send(data.to_string())
            .await
            .map_err(|e| RelayError::Adapter(e.to_string()))
    }

    /// Send SIGKILL to the child process.
    pub fn kill(&self) -> Result<(), RelayError> {
        self.killer
            .lock()
            .map_err(|_| RelayError::Adapter("killer lock poisoned".into()))?
            .kill()
            .map_err(RelayError::Io)
    }

    /// Take the output receiver. Panics if called more than once.
    pub fn take_output_rx(&mut self) -> mpsc::Receiver<String> {
        self.output_rx.take().expect("output_rx already taken")
    }
}
