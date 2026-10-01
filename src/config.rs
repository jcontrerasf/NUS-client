//! Configuración persistente (TOML) y utilidades de payload.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// UUIDs del Nordic UART Service.
pub const NUS_SERVICE: Uuid = Uuid::from_u128(0x6E400001_B5A3_F393_E0A9_E50E24DCCA9E);
/// RX del periférico: aquí escribe el cliente.
pub const NUS_RX_WRITE: Uuid = Uuid::from_u128(0x6E400002_B5A3_F393_E0A9_E50E24DCCA9E);
/// TX del periférico: notificaciones hacia el cliente.
pub const NUS_TX_NOTIFY: Uuid = Uuid::from_u128(0x6E400003_B5A3_F393_E0A9_E50E24DCCA9E);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LineEnding {
    None,
    Cr,
    Lf,
    #[default]
    CrLf,
}

impl LineEnding {
    pub const ALL: [LineEnding; 4] = [Self::None, Self::Cr, Self::Lf, Self::CrLf];

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::None => b"",
            Self::Cr => b"\r",
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Ninguno",
            Self::Cr => "CR",
            Self::Lf => "LF",
            Self::CrLf => "CR+LF",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MacroMode {
    /// Se envía directamente al pulsar el botón.
    #[default]
    Send,
    /// Se copia a la entrada de texto para editarlo antes de enviar.
    Insert,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Macro {
    pub name: String,
    pub text: String,
    pub mode: MacroMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub line_ending: LineEnding,
    /// Tamaño de trozo para escrituras. 0 = automático (MTU - 3).
    pub chunk_size: usize,
    pub write_with_response: bool,
    pub timestamps: bool,
    pub echo: bool,
    pub only_nus: bool,
    /// Característica de escritura preferida al conectar.
    pub default_write_uuid: Uuid,
    /// Característica de notificación preferida al conectar.
    pub default_notify_uuid: Uuid,
    pub macros: Vec<Macro>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            line_ending: LineEnding::CrLf,
            chunk_size: 0,
            write_with_response: false,
            timestamps: true,
            echo: true,
            only_nus: false,
            default_write_uuid: NUS_RX_WRITE,
            default_notify_uuid: NUS_TX_NOTIFY,
            macros: vec![
                Macro {
                    name: "Hola".into(),
                    text: "hello".into(),
                    mode: MacroMode::Send,
                },
                Macro {
                    name: "AT".into(),
                    text: "AT".into(),
                    mode: MacroMode::Send,
                },
                Macro {
                    name: "AT+…".into(),
                    text: "AT+".into(),
                    mode: MacroMode::Insert,
                },
            ],
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "nus-client")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// Carga la configuración. Si no existe devuelve la de por defecto; si está
    /// corrupta la respalda como `config.toml.bak` y devuelve un aviso.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (
                Self::default(),
                Some("No se pudo determinar el directorio de configuración".into()),
            );
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => match Self::from_toml(&s) {
                Ok(cfg) => (cfg, None),
                Err(e) => {
                    let bak = path.with_extension("toml.bak");
                    let _ = std::fs::rename(&path, &bak);
                    (
                        Self::default(),
                        Some(format!(
                            "Config inválida ({e}); respaldada en {}",
                            bak.display()
                        )),
                    )
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("No se pudo leer {}: {e}", path.display())),
            ),
        }
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::path().context("sin directorio de configuración")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, self.to_toml()?)?;
        Ok(path)
    }

    pub fn from_toml(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// Texto + fin de línea.
pub fn build_payload(text: &str, ending: LineEnding) -> Vec<u8> {
    let mut v = text.as_bytes().to_vec();
    v.extend_from_slice(ending.bytes());
    v
}

/// Tamaño de trozo efectivo: el configurado, o MTU-3 si es automático.
pub fn effective_chunk_size(configured: usize, mtu: u16) -> usize {
    if configured > 0 {
        configured.clamp(1, 512)
    } else {
        (mtu as usize).saturating_sub(3).clamp(20, 512)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let mut cfg = Config {
            line_ending: LineEnding::Lf,
            chunk_size: 100,
            ..Default::default()
        };
        cfg.macros.push(Macro {
            name: "x".into(),
            text: "y\"z".into(),
            mode: MacroMode::Insert,
        });
        let s = cfg.to_toml().unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), cfg);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let cfg =
            Config::from_toml("line_ending = \"CR\"\n[[macros]]\nname = \"A\"\ntext = \"B\"\n")
                .unwrap();
        assert_eq!(cfg.line_ending, LineEnding::Cr);
        assert_eq!(cfg.default_write_uuid, NUS_RX_WRITE);
        assert_eq!(cfg.macros.len(), 1);
        assert_eq!(cfg.macros[0].mode, MacroMode::Send);
    }

    #[test]
    fn payload_line_endings() {
        assert_eq!(build_payload("ab", LineEnding::None), b"ab");
        assert_eq!(build_payload("ab", LineEnding::Cr), b"ab\r");
        assert_eq!(build_payload("ab", LineEnding::Lf), b"ab\n");
        assert_eq!(build_payload("ab", LineEnding::CrLf), b"ab\r\n");
    }

    #[test]
    fn chunk_sizes() {
        assert_eq!(effective_chunk_size(0, 23), 20);
        assert_eq!(effective_chunk_size(0, 247), 244);
        assert_eq!(effective_chunk_size(0, 0), 20);
        assert_eq!(effective_chunk_size(50, 247), 50);
        assert_eq!(effective_chunk_size(9999, 23), 512);
    }
}
