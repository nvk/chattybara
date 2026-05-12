use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const DEFAULT_RIGCTLD_HOST: &str = "127.0.0.1:4532";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HamlibConfig {
    pub host: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HamlibPttState {
    Rx,
    Tx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HamlibFrequencyReport {
    pub kind: &'static str,
    pub host: String,
    pub frequency_hz: u64,
    pub raw: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HamlibModeReport {
    pub kind: &'static str,
    pub host: String,
    pub mode: String,
    pub passband_hz: u64,
    pub raw: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HamlibPttReport {
    pub kind: &'static str,
    pub host: String,
    pub state: &'static str,
    pub raw: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HamlibStatusReport {
    pub kind: &'static str,
    pub host: String,
    pub ok: bool,
    pub frequency_hz: Option<u64>,
    pub mode: Option<String>,
    pub passband_hz: Option<u64>,
    pub ptt: Option<String>,
    pub errors: Vec<String>,
}

pub fn hamlib_get_frequency(config: &HamlibConfig) -> Result<HamlibFrequencyReport> {
    let mut client = RigctldClient::connect(config)?;
    let raw = client.get_lines("f", 1)?;
    let frequency_hz = raw[0]
        .trim()
        .parse::<u64>()
        .with_context(|| format!("parsing rigctld frequency {:?}", raw[0]))?;
    Ok(HamlibFrequencyReport {
        kind: "hamlib-frequency-report",
        host: config.host.clone(),
        frequency_hz,
        raw,
    })
}

pub fn hamlib_get_mode(config: &HamlibConfig) -> Result<HamlibModeReport> {
    let mut client = RigctldClient::connect(config)?;
    let raw = client.get_lines("m", 2)?;
    let mode = raw[0].trim().to_owned();
    let passband_hz = raw[1]
        .trim()
        .parse::<u64>()
        .with_context(|| format!("parsing rigctld passband {:?}", raw[1]))?;
    Ok(HamlibModeReport {
        kind: "hamlib-mode-report",
        host: config.host.clone(),
        mode,
        passband_hz,
        raw,
    })
}

pub fn hamlib_get_ptt(config: &HamlibConfig) -> Result<HamlibPttReport> {
    let mut client = RigctldClient::connect(config)?;
    let raw = client.get_lines("t", 1)?;
    Ok(HamlibPttReport {
        kind: "hamlib-ptt-report",
        host: config.host.clone(),
        state: ptt_label(raw[0].trim()),
        raw,
    })
}

pub fn hamlib_set_ptt(config: &HamlibConfig, state: HamlibPttState) -> Result<HamlibPttReport> {
    let mut client = RigctldClient::connect(config)?;
    let command = match state {
        HamlibPttState::Rx => "T 0",
        HamlibPttState::Tx => "T 1",
    };
    let raw = client.set(command)?;
    Ok(HamlibPttReport {
        kind: "hamlib-ptt-report",
        host: config.host.clone(),
        state: match state {
            HamlibPttState::Rx => "rx",
            HamlibPttState::Tx => "tx",
        },
        raw,
    })
}

pub fn hamlib_status(config: &HamlibConfig) -> HamlibStatusReport {
    let mut errors = Vec::new();
    let frequency_hz = match hamlib_get_frequency(config) {
        Ok(report) => Some(report.frequency_hz),
        Err(error) => {
            errors.push(format!("frequency: {error}"));
            None
        }
    };
    let (mode, passband_hz) = match hamlib_get_mode(config) {
        Ok(report) => (Some(report.mode), Some(report.passband_hz)),
        Err(error) => {
            errors.push(format!("mode: {error}"));
            (None, None)
        }
    };
    let ptt = match hamlib_get_ptt(config) {
        Ok(report) => Some(report.state.to_owned()),
        Err(error) => {
            errors.push(format!("ptt: {error}"));
            None
        }
    };
    HamlibStatusReport {
        kind: "hamlib-status-report",
        host: config.host.clone(),
        ok: errors.is_empty(),
        frequency_hz,
        mode,
        passband_hz,
        ptt,
        errors,
    }
}

struct RigctldClient {
    reader: BufReader<TcpStream>,
}

impl RigctldClient {
    fn connect(config: &HamlibConfig) -> Result<Self> {
        if config.timeout_ms == 0 {
            bail!("rigctld timeout must be greater than zero");
        }
        let stream = TcpStream::connect(&config.host)
            .with_context(|| format!("connecting to rigctld {}", config.host))?;
        let timeout = Some(Duration::from_millis(config.timeout_ms));
        stream
            .set_read_timeout(timeout)
            .context("setting rigctld read timeout")?;
        stream
            .set_write_timeout(timeout)
            .context("setting rigctld write timeout")?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    fn get_lines(&mut self, command: &str, expected_lines: usize) -> Result<Vec<String>> {
        self.write_command(command)?;
        let mut lines = Vec::with_capacity(expected_lines);
        while lines.len() < expected_lines {
            let line = self.read_line(command)?;
            if is_rprt_error(&line) {
                bail!("rigctld command {command:?} failed: {line}");
            }
            lines.push(line);
        }
        Ok(lines)
    }

    fn set(&mut self, command: &str) -> Result<Vec<String>> {
        self.write_command(command)?;
        let line = self.read_line(command)?;
        if line.trim() != "RPRT 0" {
            bail!("rigctld command {command:?} failed: {line}");
        }
        Ok(vec![line])
    }

    fn write_command(&mut self, command: &str) -> Result<()> {
        let stream = self.reader.get_mut();
        stream
            .write_all(command.as_bytes())
            .with_context(|| format!("writing rigctld command {command:?}"))?;
        stream
            .write_all(b"\n")
            .with_context(|| format!("terminating rigctld command {command:?}"))?;
        stream.flush().context("flushing rigctld command")
    }

    fn read_line(&mut self, command: &str) -> Result<String> {
        let mut line = String::new();
        let count = self
            .reader
            .read_line(&mut line)
            .with_context(|| format!("reading rigctld response to {command:?}"))?;
        if count == 0 {
            bail!("rigctld closed connection while handling {command:?}");
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

fn is_rprt_error(line: &str) -> bool {
    line.trim()
        .strip_prefix("RPRT ")
        .and_then(|value| value.parse::<i32>().ok())
        .is_some_and(|code| code < 0)
}

fn ptt_label(value: &str) -> &'static str {
    match value {
        "0" => "rx",
        "1" => "tx",
        "2" => "tx-mic",
        "3" => "tx-data",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn hamlib_status_reads_simple_rigctld_protocol() {
        let host = spawn_fake_rigctld(vec![
            ("f".to_owned(), vec!["14074000".to_owned()]),
            ("m".to_owned(), vec!["USB".to_owned(), "2400".to_owned()]),
            ("t".to_owned(), vec!["0".to_owned()]),
        ]);
        let report = hamlib_status(&HamlibConfig {
            host,
            timeout_ms: 1000,
        });

        assert!(report.ok);
        assert_eq!(report.frequency_hz, Some(14_074_000));
        assert_eq!(report.mode.as_deref(), Some("USB"));
        assert_eq!(report.passband_hz, Some(2400));
        assert_eq!(report.ptt.as_deref(), Some("rx"));
    }

    #[test]
    fn hamlib_ptt_tx_sends_set_command() {
        let host = spawn_fake_rigctld(vec![("T 1".to_owned(), vec!["RPRT 0".to_owned()])]);
        let report = hamlib_set_ptt(
            &HamlibConfig {
                host,
                timeout_ms: 1000,
            },
            HamlibPttState::Tx,
        )
        .expect("ptt tx");

        assert_eq!(report.state, "tx");
        assert_eq!(report.raw, vec!["RPRT 0"]);
    }

    fn spawn_fake_rigctld(commands: Vec<(String, Vec<String>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rigctld");
        let address = listener.local_addr().expect("addr").to_string();
        thread::spawn(move || {
            for (expected, replies) in commands {
                let (stream, _) = listener.accept().expect("accept");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut line = String::new();
                reader.read_line(&mut line).expect("read command");
                assert_eq!(line.trim_end(), expected);
                let mut stream = reader.into_inner();
                for reply in replies {
                    writeln!(stream, "{reply}").expect("reply");
                }
            }
        });
        address
    }
}
