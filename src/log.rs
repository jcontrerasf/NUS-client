//! Log de sesión a archivo con timestamps.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

pub struct SessionLog {
    path: PathBuf,
    out: BufWriter<File>,
}

impl SessionLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            out: BufWriter::new(file),
        })
    }

    /// Ruta por defecto: `<datos de usuario>/nus-client/logs/session-FECHA.log`.
    pub fn default_path() -> PathBuf {
        let dir = directories::ProjectDirs::from("", "", "nus-client")
            .map(|d| d.data_dir().join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"));
        dir.join(format!(
            "session-{}.log",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_line(&mut self, tag: &str, text: &str) -> Result<()> {
        writeln!(
            self.out,
            "[{}] {tag}: {text}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
        )?;
        self.out.flush()?;
        Ok(())
    }
}
