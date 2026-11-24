//! Form values only. Deserialization never restores resources or TX authority.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use wsjt_rusty::ft8::transmit;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PttMethod {
    #[default]
    None,
    Cat,
    Rts,
    Dtr,
    SeparateRts,
    SeparateDtr,
}
impl PttMethod {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Rts,
        Self::Dtr,
        Self::SeparateRts,
        Self::SeparateDtr,
        Self::Cat,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No PTT configured",
            Self::Cat => "CAT · newer IC-718 #82/#83 only",
            Self::Rts => "RTS · same CI-V port",
            Self::Dtr => "DTR · same CI-V port",
            Self::SeparateRts => "RTS · separate PTT port",
            Self::SeparateDtr => "DTR · separate PTT port",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub callsign: String,
    pub grid: String,
    pub output: String,
    pub attenuation: f32,
    pub use_radio: bool,
    pub auto_add: bool,
    pub expire: bool,
    pub timeout: u32,
    pub sort: [bool; 3],
    pub open_frequency: bool,
    pub port: String,
    pub ptt_port: String,
    pub baud: u32,
    pub address: String,
    pub ptt: PttMethod,
    pub frequency: u64,
    pub mode: u8,
    pub power: u8,
    pub exclude_days: Option<u32>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            callsign: String::new(),
            grid: String::new(),
            output: String::new(),
            attenuation: 20.0,
            use_radio: true,
            auto_add: false,
            expire: true,
            timeout: 180,
            sort: [true, false, false],
            open_frequency: false,
            port: String::new(),
            ptt_port: String::new(),
            baud: 19200,
            address: "5E".into(),
            ptt: PttMethod::None,
            frequency: 14_074_000,
            mode: 1,
            power: 25,
            exclude_days: None,
        }
    }
}
impl Preferences {
    pub fn validate_station(&self) -> Result<(), String> {
        transmit::pack_call(&self.callsign).map_err(str::to_string)?;
        transmit::grid_index(&self.grid).map_err(str::to_string)?;
        if self.output.is_empty() {
            return Err("Select the radio's transmit audio output.".into());
        }
        Ok(())
    }
    pub fn address(&self) -> Result<u8, String> {
        u8::from_str_radix(self.address.trim(), 16)
            .ok()
            .filter(|a| (1..=127).contains(a))
            .ok_or_else(|| "CI-V address must be 01–7F hex.".into())
    }
    pub fn validate_radio(&self) -> Result<(), String> {
        self.address()?;
        if !wsjt_rig_native::BAUD_RATES.contains(&self.baud) {
            return Err("Unsupported radio baud rate.".into());
        }
        if self.port.trim().is_empty() {
            return Err("Select a CI-V serial port.".into());
        }
        if matches!(self.ptt, PttMethod::SeparateRts | PttMethod::SeparateDtr)
            && (self.ptt_port.trim().is_empty() || self.ptt_port == self.port)
        {
            return Err(
                "Select a different port for separate PTT, or choose same-port RTS/DTR.".into(),
            );
        }
        Ok(())
    }
    pub fn load() -> (Self, Option<String>) {
        let result: Result<(Self, bool), String> = (|| {
            let path = path()?;
            if !path.exists() {
                return Ok((Self::default(), false));
            }
            let mut p: Self =
                serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let mut invalid = false;
            if p.exclude_days.is_some_and(|days| days > 36500) {
                p.exclude_days = None;
                invalid = true;
            }
            p.callsign = p.callsign.trim().to_ascii_uppercase();
            p.grid = p.grid.trim().to_ascii_uppercase();
            if !p.callsign.is_empty() && transmit::pack_call(&p.callsign).is_err() {
                p.callsign.clear();
                invalid = true;
            }
            if !p.grid.is_empty() && transmit::grid_index(&p.grid).is_err() {
                p.grid.clear();
                invalid = true;
            }
            if !p.attenuation.is_finite() || !(0.0..=60.0).contains(&p.attenuation) {
                p.attenuation = 60.0;
                invalid = true;
            }
            if !(1..=86400).contains(&p.timeout) {
                p.timeout = 180;
                invalid = true;
            }
            if !wsjt_rig_native::BAUD_RATES.contains(&p.baud) {
                p.baud = 19200;
                invalid = true;
            }
            if p.address().is_err() {
                p.address = "5E".into();
                invalid = true;
            }
            if !(30_000..=29_999_999).contains(&p.frequency) {
                p.frequency = 14_074_000;
                invalid = true;
            }
            if wsjt_rig_native::Mode::from_byte(p.mode).is_err() {
                p.mode = 1;
                invalid = true;
            }
            Ok((p, invalid))
        })();
        match result {
            Ok((p, invalid)) => (
                p,
                invalid.then(|| {
                    "Some saved settings were invalid and reset; resources remain disconnected."
                        .into()
                }),
            ),
            Err(error) => (
                Self::default(),
                Some(format!("Could not restore preferences: {error}")),
            ),
        }
    }
    pub fn save(&self) -> Result<(), String> {
        let path = path()?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(temporary, path).map_err(|e| e.to_string())
    }
}
fn path() -> Result<PathBuf, String> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
    };
    base.map(|p| p.join("wsjt-rusty").join("native-settings.json"))
        .ok_or_else(|| "No user configuration directory available.".into())
}
pub fn directory() -> Result<PathBuf, String> {
    path().map(|p| p.parent().unwrap().to_path_buf())
}
