//! Spawns a child process attached to a pseudo console (ConPTY on Windows).
//!
//! The program path must be absolute so the current directory cannot shadow it; arguments are passed as an array and
//! never joined into a shell string by us.
#![forbid(unsafe_code)]

use std::{
    io::{Read, Write},
    path::PathBuf,
};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Errors from spawning or controlling the pseudo console.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The program path is not absolute.
    #[error("program path must be absolute: {0}")]
    RelativeProgram(PathBuf),
    /// The pseudo console or child process failed.
    #[error("pty: {0}")]
    Pty(String),
    /// I/O error while waiting for or killing the child.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

fn pty_error(e: impl std::fmt::Display) -> Error {
    Error::Pty(e.to_string())
}

/// What to run.
#[derive(Debug, Clone, Default)]
pub struct SpawnConfig {
    /// Absolute path of the executable.
    pub program: PathBuf,
    /// Arguments, passed as-is.
    pub args: Vec<String>,
    /// Working directory; `None` inherits ours.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables on top of the inherited environment.
    pub env: Vec<(String, String)>,
}

/// Grid size of the pseudo console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PtyDimensions {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Cell width in pixels (0 if unknown).
    pub cell_width_px: u16,
    /// Cell height in pixels (0 if unknown).
    pub cell_height_px: u16,
}

impl PtyDimensions {
    fn to_portable(self) -> PtySize {
        PtySize {
            rows: self.rows.max(1),
            cols: self.cols.max(1),
            pixel_width: self.cols.saturating_mul(self.cell_width_px),
            pixel_height: self.rows.saturating_mul(self.cell_height_px),
        }
    }
}

/// A spawned child with the I/O ends of its pseudo console.
pub struct Spawned {
    /// Control handle (resize).
    pub pty: Pty,
    /// Program output.
    pub reader: Box<dyn Read + Send>,
    /// Program input.
    pub writer: Box<dyn Write + Send>,
    /// The child process.
    pub child: ChildProcess,
}

impl std::fmt::Debug for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spawned").field("child", &self.child).finish_non_exhaustive()
    }
}

/// Control handle of a pseudo console.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty").finish_non_exhaustive()
    }
}

impl Pty {
    /// Resizes the pseudo console.
    pub fn resize(&self, size: PtyDimensions) -> Result<()> {
        self.master.resize(size.to_portable()).map_err(pty_error)
    }
}

/// A child process running in a pseudo console.
pub struct ChildProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl std::fmt::Debug for ChildProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildProcess").field("pid", &self.child.process_id()).finish()
    }
}

impl ChildProcess {
    /// Blocks until the child exits; returns its exit code.
    pub fn wait(&mut self) -> Result<u32> {
        Ok(self.child.wait()?.exit_code())
    }

    /// A handle that can kill the child from another thread.
    pub fn killer(&self) -> Killer {
        Killer(self.child.clone_killer())
    }
}

/// Kills a child process; usable from any thread.
pub struct Killer(Box<dyn ChildKiller + Send + Sync>);

impl std::fmt::Debug for Killer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Killer").finish_non_exhaustive()
    }
}

impl Killer {
    /// Terminates the child.
    pub fn kill(&mut self) -> Result<()> {
        Ok(self.0.kill()?)
    }
}

/// Starts `config.program` in a new pseudo console of `size`.
pub fn spawn(config: &SpawnConfig, size: PtyDimensions) -> Result<Spawned> {
    if !config.program.is_absolute() {
        return Err(Error::RelativeProgram(config.program.clone()));
    }
    let pair = native_pty_system().openpty(size.to_portable()).map_err(pty_error)?;
    let mut cmd = CommandBuilder::new(&config.program);
    cmd.args(&config.args);
    if let Some(cwd) = &config.cwd {
        cmd.cwd(cwd);
    }
    for (key, value) in &config.env {
        cmd.env(key, value);
    }
    let child = pair.slave.spawn_command(cmd).map_err(pty_error)?;
    // The slave end must be closed here, or reads never see EOF after the child exits.
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(pty_error)?;
    let writer = pair.master.take_writer().map_err(pty_error)?;
    Ok(Spawned { pty: Pty { master: pair.master }, reader, writer, child: ChildProcess { child } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_program() {
        let config = SpawnConfig { program: PathBuf::from("powershell.exe"), ..SpawnConfig::default() };
        let size = PtyDimensions { cols: 80, rows: 24, ..PtyDimensions::default() };
        assert!(matches!(spawn(&config, size), Err(Error::RelativeProgram(_))));
    }
}
