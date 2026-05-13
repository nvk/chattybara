use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chattybara_station::{
    AdapterHealthEvent, DecodeEvent, DirectedMessageEvent, ModeId, RigStatusEvent, SpotEvent,
    StationEvent, StationLogRecord, replay_summary, write_event_log,
};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WSJTX_MAGIC: u32 = 0xadbccbda;
const WSJTX_DECODE_TYPE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAdapterId {
    Js8call,
    Wsjtx,
    Fldigi,
    CwAssist,
    PskReporter,
}

impl ExternalAdapterId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Js8call => "js8call",
            Self::Wsjtx => "wsjtx",
            Self::Fldigi => "fldigi",
            Self::CwAssist => "cw-assist",
            Self::PskReporter => "pskreporter",
        }
    }

    pub fn mode(self) -> ModeId {
        match self {
            Self::Js8call => ModeId::Js8callExternal,
            Self::Wsjtx => ModeId::WsjtxExternal,
            Self::Fldigi => ModeId::FldigiExternal,
            Self::CwAssist => ModeId::CwAssist,
            Self::PskReporter => ModeId::PskReporter,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExternalAdapterRunConfig {
    pub adapter: ExternalAdapterId,
    pub station: String,
    pub host: String,
    pub port: u16,
    pub protocol_kind: &'static str,
    pub protocol_label: &'static str,
    pub live: bool,
    pub timeout_ms: u64,
    pub max_events: usize,
    pub enable_tx: bool,
    pub enable_reporting: bool,
    pub allow_transmit: bool,
    pub send_to: Option<String>,
    pub message: Option<String>,
    pub fixture: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub psk_scheme: String,
    pub psk_path: String,
    pub psk_query_call: Option<String>,
    pub psk_lookback_seconds: u32,
}

pub fn run_external_adapter(config: ExternalAdapterRunConfig) -> Result<Value> {
    if config.timeout_ms == 0 {
        bail!("adapter timeout must be greater than zero");
    }
    if config.max_events == 0 {
        bail!("adapter max events must be greater than zero");
    }
    if config.enable_tx && !config.allow_transmit {
        bail!("transmit requested without --allow-transmit");
    }

    let mut notes = Vec::new();
    let mut sent_commands = Vec::new();
    let records = match config.adapter {
        ExternalAdapterId::Js8call => run_js8call(&config, &mut notes, &mut sent_commands)?,
        ExternalAdapterId::Wsjtx => run_wsjtx(&config, &mut notes)?,
        ExternalAdapterId::Fldigi => run_fldigi(&config, &mut notes, &mut sent_commands)?,
        ExternalAdapterId::CwAssist => run_cw_assist(&config, &mut notes)?,
        ExternalAdapterId::PskReporter => run_pskreporter(&config, &mut notes)?,
    };

    if let Some(path) = &config.out {
        write_event_log(path, &records)?;
    }

    let summary = replay_summary(&records);
    Ok(json!({
        "kind": "station-external-adapter-live-report",
        "ok": true,
        "mode": config.adapter.mode().label(),
        "adapter": config.adapter.label(),
        "station": config.station,
        "live": config.live,
        "fixture": config.fixture,
        "out": config.out,
        "protocol": {
            "kind": config.protocol_kind,
            "label": config.protocol_label,
            "live_status": if config.live { "live" } else { "fixture" },
        },
        "endpoint": {
            "host": config.host,
            "port": config.port,
        },
        "receive_only": !config.enable_tx,
        "tx_enabled": config.enable_tx,
        "reporting_enabled": config.enable_reporting,
        "sent_commands": sent_commands,
        "event_count": records.len(),
        "summary": summary,
        "records": records,
        "notes": notes,
    }))
}

fn run_js8call(
    config: &ExternalAdapterRunConfig,
    notes: &mut Vec<String>,
    sent_commands: &mut Vec<Value>,
) -> Result<Vec<StationLogRecord>> {
    let mode = config.adapter.mode();
    let mut events = vec![StationEvent::AdapterHealth(AdapterHealthEvent {
        mode,
        ok: true,
        message: if config.live {
            "JS8Call TCP API connected".to_owned()
        } else {
            "JS8Call fixture replay loaded".to_owned()
        },
        receive_only: !config.enable_tx,
    })];

    if let Some(path) = &config.fixture {
        let raw =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Value>(line) {
                Ok(value) => {
                    if let Some(event) = js8call_value_to_event(&value, &config.station) {
                        events.push(event);
                    }
                }
                Err(error) => events.push(StationEvent::ModeError(
                    chattybara_station::ModeErrorEvent {
                        mode,
                        message: format!("invalid JS8Call JSON fixture line: {error}"),
                        recoverable: true,
                    },
                )),
            }
        }
        notes.push("loaded newline-delimited JS8Call JSON fixture".to_owned());
        return Ok(records_from_events(events));
    }

    if !config.live {
        notes.push("dry-run adapter report; pass --live or --fixture to receive events".to_owned());
        return Ok(records_from_events(events));
    }

    let timeout = Duration::from_millis(config.timeout_ms);
    let mut stream = connect_tcp(&config.host, config.port, timeout)
        .with_context(|| format!("connecting JS8Call API {}:{}", config.host, config.port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let status = json!({
        "type": "STATION.GET_STATUS",
        "value": "",
        "params": { "_ID": request_id() },
    });
    write_json_line(&mut stream, &status)?;
    sent_commands.push(status);

    if config.enable_tx {
        let to = required_text(config.send_to.as_deref(), "--send-to")?;
        let message = required_text(config.message.as_deref(), "--message")?;
        let command = json!({
            "type": "TX.SEND_MESSAGE",
            "value": format!("@{to} {message}"),
            "params": {
                "_ID": request_id(),
                "TO": to,
                "TEXT": message,
            },
        });
        write_json_line(&mut stream, &command)?;
        sent_commands.push(command);
    }

    let mut reader = BufReader::new(stream);
    while events.len() < config.max_events {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.trim().is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(&line)
                    .with_context(|| format!("parsing JS8Call JSON line {line:?}"))?;
                if let Some(event) = js8call_value_to_event(&value, &config.station) {
                    events.push(event);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                notes.push("JS8Call receive timed out after available events".to_owned());
                break;
            }
            Err(error) => return Err(error).context("reading JS8Call API"),
        }
    }
    Ok(records_from_events(events))
}

fn run_wsjtx(
    config: &ExternalAdapterRunConfig,
    notes: &mut Vec<String>,
) -> Result<Vec<StationLogRecord>> {
    let mode = config.adapter.mode();
    let mut events = vec![StationEvent::AdapterHealth(AdapterHealthEvent {
        mode,
        ok: true,
        message: if config.live {
            "WSJT-X UDP listener ready".to_owned()
        } else {
            "WSJT-X fixture replay loaded".to_owned()
        },
        receive_only: true,
    })];

    if let Some(path) = &config.fixture {
        let raw = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let datagrams = wsjtx_fixture_datagrams(&raw)?;
        for datagram in datagrams {
            if let Some(event) = parse_wsjtx_datagram(&datagram)? {
                events.push(event);
            }
        }
        notes.push("loaded WSJT-X UDP datagram fixture".to_owned());
        return Ok(records_from_events(events));
    }

    if !config.live {
        notes
            .push("dry-run adapter report; pass --live or --fixture to receive decodes".to_owned());
        return Ok(records_from_events(events));
    }

    let socket =
        UdpSocket::bind(format!("{}:{}", config.host, config.port)).with_context(|| {
            format!(
                "binding WSJT-X UDP listener {}:{}",
                config.host, config.port
            )
        })?;
    socket.set_read_timeout(Some(Duration::from_millis(config.timeout_ms)))?;
    let mut buffer = vec![0_u8; 8192];
    while events.len() < config.max_events {
        match socket.recv_from(&mut buffer) {
            Ok((len, _source)) => {
                if let Some(event) = parse_wsjtx_datagram(&buffer[..len])? {
                    events.push(event);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                notes.push("WSJT-X receive timed out after available datagrams".to_owned());
                break;
            }
            Err(error) => return Err(error).context("reading WSJT-X UDP datagram"),
        }
    }
    Ok(records_from_events(events))
}

fn run_fldigi(
    config: &ExternalAdapterRunConfig,
    notes: &mut Vec<String>,
    sent_commands: &mut Vec<Value>,
) -> Result<Vec<StationLogRecord>> {
    let mode = config.adapter.mode();
    let mut events = vec![StationEvent::AdapterHealth(AdapterHealthEvent {
        mode,
        ok: true,
        message: if config.live {
            "fldigi XML-RPC connected".to_owned()
        } else {
            "fldigi fixture replay loaded".to_owned()
        },
        receive_only: !config.enable_tx,
    })];

    if let Some(path) = &config.fixture {
        let raw =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if !raw.trim().is_empty() {
            events.push(StationEvent::Decode(DecodeEvent {
                mode,
                from: first_callsign(&raw),
                text: raw.trim().to_owned(),
                snr_db: None,
                frequency_hz: None,
            }));
        }
        notes.push("loaded fldigi RX text fixture".to_owned());
        return Ok(records_from_events(events));
    }

    if !config.live {
        notes.push("dry-run adapter report; pass --live or --fixture to receive text".to_owned());
        return Ok(records_from_events(events));
    }

    let timeout = Duration::from_millis(config.timeout_ms);
    let name = xmlrpc_call(
        &config.host,
        config.port,
        timeout,
        "fldigi.name_version",
        &[],
    )
    .context("calling fldigi.name_version")?;
    notes.push(format!("fldigi reports {name}"));

    let rx = xmlrpc_call(&config.host, config.port, timeout, "rx.get_data", &[])
        .context("calling rx.get_data")?;
    if !rx.trim().is_empty() {
        events.push(StationEvent::Decode(DecodeEvent {
            mode,
            from: first_callsign(&rx),
            text: rx.trim().to_owned(),
            snr_db: None,
            frequency_hz: None,
        }));
    }

    if config.enable_tx {
        let message = required_text(config.message.as_deref(), "--message")?;
        let added = xmlrpc_call(
            &config.host,
            config.port,
            timeout,
            "text.add_tx",
            &[message],
        )
        .context("calling text.add_tx")?;
        sent_commands.push(json!({"method": "text.add_tx", "result": added}));
        let tx = xmlrpc_call(&config.host, config.port, timeout, "main.tx", &[])
            .context("calling main.tx")?;
        sent_commands.push(json!({"method": "main.tx", "result": tx}));
    }

    Ok(records_from_events(events))
}

fn run_cw_assist(
    config: &ExternalAdapterRunConfig,
    notes: &mut Vec<String>,
) -> Result<Vec<StationLogRecord>> {
    let mode = config.adapter.mode();
    let mut events = vec![StationEvent::AdapterHealth(AdapterHealthEvent {
        mode,
        ok: true,
        message: "CW assist receive-only fixture decoder ready".to_owned(),
        receive_only: true,
    })];
    let raw = match &config.fixture {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => "CQ TEST JA1QSO".to_owned(),
    };
    let text = if looks_like_morse(&raw) {
        decode_morse_fixture(&raw)
    } else {
        raw.trim().to_owned()
    };
    if !text.is_empty() {
        events.push(StationEvent::Decode(DecodeEvent {
            mode,
            from: first_callsign(&text),
            text,
            snr_db: None,
            frequency_hz: None,
        }));
    }
    notes.push("CW assist is receive-only; no transmit command is exposed".to_owned());
    Ok(records_from_events(events))
}

fn run_pskreporter(
    config: &ExternalAdapterRunConfig,
    notes: &mut Vec<String>,
) -> Result<Vec<StationLogRecord>> {
    let mode = config.adapter.mode();
    let mut events = vec![StationEvent::AdapterHealth(AdapterHealthEvent {
        mode,
        ok: true,
        message: if config.live {
            "PSK Reporter query completed".to_owned()
        } else {
            "PSK Reporter fixture replay loaded".to_owned()
        },
        receive_only: true,
    })];

    let xml = if let Some(path) = &config.fixture {
        notes.push("loaded PSK Reporter XML fixture".to_owned());
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else if config.live {
        let query_call = config
            .psk_query_call
            .as_deref()
            .unwrap_or(config.station.as_str());
        let url = format!(
            "{}://{}:{}{}?callsign={}&flowStartSeconds=-{}",
            config.psk_scheme,
            config.host,
            config.port,
            config.psk_path,
            percent_encode(query_call),
            config.psk_lookback_seconds
        );
        notes.push(format!("queried {url}"));
        ureq::AgentBuilder::new()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .get(&url)
            .call()
            .map_err(|error| anyhow::anyhow!("PSK Reporter query failed: {error}"))?
            .into_string()
            .context("reading PSK Reporter response")?
    } else {
        notes.push("dry-run adapter report; pass --live or --fixture to receive spots".to_owned());
        String::new()
    };

    for attrs in parse_reception_report_attrs(&xml) {
        let call = attrs
            .get("senderCallsign")
            .or_else(|| attrs.get("receiverCallsign"))
            .cloned()
            .unwrap_or_else(|| "UNKNOWN".to_owned());
        let frequency_hz = attrs
            .get("frequency")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let snr_db = attrs
            .get("sNR")
            .or_else(|| attrs.get("snr"))
            .and_then(|value| value.parse::<i16>().ok());
        events.push(StationEvent::Spot(SpotEvent {
            mode,
            call_sign: call,
            frequency_hz,
            snr_db,
            source: "pskreporter".to_owned(),
        }));
        if events.len() >= config.max_events {
            break;
        }
    }
    Ok(records_from_events(events))
}

fn connect_tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    let endpoint = format!("{host}:{port}");
    let address = endpoint
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("could not resolve {endpoint}"))?;
    TcpStream::connect_timeout(&address, timeout).map_err(Into::into)
}

fn write_json_line(stream: &mut TcpStream, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stream, value)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn js8call_value_to_event(value: &Value, station: &str) -> Option<StationEvent> {
    let typ = value.get("type")?.as_str().unwrap_or_default();
    let params = value.get("params").unwrap_or(&Value::Null);
    let text = value
        .get("value")
        .and_then(Value::as_str)
        .or_else(|| params.get("TEXT").and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_owned();
    match typ {
        "RX.DIRECTED" | "RX.DIRECTED.ME" => {
            Some(StationEvent::DirectedMessage(DirectedMessageEvent {
                mode: ModeId::Js8callExternal,
                from: json_str(params, &["FROM", "CALL", "SENDER"])
                    .unwrap_or("UNKNOWN")
                    .to_owned(),
                to: json_str(params, &["TO", "DESTINATION"])
                    .unwrap_or(station)
                    .to_owned(),
                text,
                snr_db: json_i16(params, &["SNR"]),
            }))
        }
        "RX.SPOT" => Some(StationEvent::Spot(SpotEvent {
            mode: ModeId::Js8callExternal,
            call_sign: json_str(params, &["CALL", "FROM"])
                .unwrap_or("UNKNOWN")
                .to_owned(),
            frequency_hz: json_u64(params, &["FREQ"])
                .or_else(|| {
                    Some(json_u64(params, &["DIAL"])? + json_u64(params, &["OFFSET"]).unwrap_or(0))
                })
                .unwrap_or(0),
            snr_db: json_i16(params, &["SNR"]),
            source: "js8call".to_owned(),
        })),
        "RX.ACTIVITY" | "RX.TEXT" => Some(StationEvent::Decode(DecodeEvent {
            mode: ModeId::Js8callExternal,
            from: first_callsign(&text),
            text,
            snr_db: json_i16(params, &["SNR"]),
            frequency_hz: json_u64(params, &["FREQ", "DIAL"]),
        })),
        "STATION.STATUS" | "RIG.FREQ" => Some(StationEvent::RigStatus(RigStatusEvent {
            mode: ModeId::Js8callExternal,
            frequency_hz: json_u64(params, &["FREQ", "DIAL"]),
            radio: json_str(params, &["RIG"]).map(str::to_owned),
            ptt: None,
        })),
        _ if !text.is_empty() => Some(StationEvent::Decode(DecodeEvent {
            mode: ModeId::Js8callExternal,
            from: first_callsign(&text),
            text,
            snr_db: json_i16(params, &["SNR"]),
            frequency_hz: json_u64(params, &["FREQ", "DIAL"]),
        })),
        _ => None,
    }
}

fn wsjtx_fixture_datagrams(raw: &[u8]) -> Result<Vec<Vec<u8>>> {
    if let Ok(text) = std::str::from_utf8(raw) {
        let trimmed = text.trim();
        if trimmed
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch.is_ascii_whitespace())
            && trimmed.chars().any(|ch| ch.is_ascii_hexdigit())
        {
            return Ok(vec![decode_hex(trimmed)?]);
        }
    }
    Ok(vec![raw.to_vec()])
}

fn parse_wsjtx_datagram(datagram: &[u8]) -> Result<Option<StationEvent>> {
    let mut reader = ByteReader::new(datagram);
    let magic = reader.read_u32()?;
    if magic != WSJTX_MAGIC {
        bail!("invalid WSJT-X magic 0x{magic:08x}");
    }
    let _schema = reader.read_u32()?;
    let message_type = reader.read_u32()?;
    let _id = reader.read_utf8()?;
    if message_type != WSJTX_DECODE_TYPE {
        return Ok(None);
    }
    let _new = reader.read_bool()?;
    let _time_ms = reader.read_u32()?;
    let snr = reader.read_i32()? as i16;
    let _delta_time = reader.read_f64()?;
    let delta_frequency = reader.read_u32()? as u64;
    let mode = reader.read_utf8()?;
    let message = reader.read_utf8()?;
    let _low_confidence = reader.read_bool()?;
    let _off_air = reader.read_bool()?;
    Ok(Some(StationEvent::Decode(DecodeEvent {
        mode: ModeId::WsjtxExternal,
        from: first_callsign(&message),
        text: if mode.trim().is_empty() {
            message
        } else {
            format!("{mode}: {message}")
        },
        snr_db: Some(snr),
        frequency_hz: Some(delta_frequency),
    })))
}

fn xmlrpc_call(
    host: &str,
    port: u16,
    timeout: Duration,
    method: &str,
    params: &[&str],
) -> Result<String> {
    let mut body = String::from("<?xml version=\"1.0\"?><methodCall><methodName>");
    body.push_str(&xml_escape(method));
    body.push_str("</methodName><params>");
    for param in params {
        body.push_str("<param><value><string>");
        body.push_str(&xml_escape(param));
        body.push_str("</string></value></param>");
    }
    body.push_str("</params></methodCall>");

    let mut stream = connect_tcp(host, port, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = format!(
        "POST /RPC2 HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    let (status, body) = read_http_response(&mut stream)?;
    if !status.starts_with("HTTP/1.1 200") && !status.starts_with("HTTP/1.0 200") {
        bail!("fldigi XML-RPC HTTP error: {status}");
    }
    parse_xmlrpc_value(&body)
}

fn read_http_response(stream: &mut TcpStream) -> Result<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .context("reading HTTP status line")?;
    if status.trim().is_empty() {
        bail!("empty HTTP response");
    }
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("reading HTTP response header")?;
        if line.is_empty() || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length =
                Some(value.trim().parse::<usize>().with_context(|| {
                    format!("parsing HTTP content-length value {}", value.trim())
                })?);
        }
    }
    let mut body = Vec::new();
    if let Some(content_length) = content_length {
        body.resize(content_length, 0);
        reader
            .read_exact(&mut body)
            .context("reading HTTP response body")?;
    } else {
        reader
            .read_to_end(&mut body)
            .context("reading HTTP response body")?;
    }
    Ok((
        status.trim().to_owned(),
        String::from_utf8_lossy(&body).into_owned(),
    ))
}

fn parse_xmlrpc_value(xml: &str) -> Result<String> {
    if let Some(value) = tag_text(xml, "fault") {
        bail!("XML-RPC fault: {}", compact_ws(&value));
    }
    if let Some(value) = tag_text(xml, "base64") {
        let decoded = BASE64
            .decode(compact_ws(&value))
            .context("decoding XML-RPC base64 value")?;
        return Ok(String::from_utf8_lossy(&decoded).into_owned());
    }
    for tag in ["string", "double", "int", "i4", "boolean"] {
        if let Some(value) = tag_text(xml, tag) {
            return Ok(xml_unescape(&value));
        }
    }
    if let Some(value) = tag_text(xml, "value") {
        return Ok(xml_unescape(value.trim()));
    }
    Ok(String::new())
}

fn parse_reception_report_attrs(xml: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    let mut reports = Vec::new();
    let mut rest = xml;
    while let Some(index) = rest.find("<receptionReport") {
        let after_name = &rest[index + "<receptionReport".len()..];
        if !after_name
            .chars()
            .next()
            .map(|ch| ch.is_ascii_whitespace() || ch == '/' || ch == '>')
            .unwrap_or(false)
        {
            rest = &after_name[after_name.chars().next().map(char::len_utf8).unwrap_or(0)..];
            continue;
        }
        rest = after_name;
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        reports.push(parse_attrs(tag));
        rest = &rest[end + 1..];
    }
    reports
}

fn parse_attrs(tag: &str) -> std::collections::BTreeMap<String, String> {
    let mut attrs = std::collections::BTreeMap::new();
    let mut rest = tag.trim();
    while let Some(eq) = rest.find('=') {
        let key_start = rest[..eq]
            .rfind(|ch: char| ch.is_ascii_whitespace())
            .map(|index| index + 1)
            .unwrap_or(0);
        let key = rest[key_start..eq].trim().trim_start_matches('/');
        let after_eq = rest[eq + 1..].trim_start();
        let Some(quote) = after_eq
            .chars()
            .next()
            .filter(|ch| *ch == '"' || *ch == '\'')
        else {
            break;
        };
        let value_start = quote.len_utf8();
        let Some(value_end) = after_eq[value_start..].find(quote) else {
            break;
        };
        let value = &after_eq[value_start..value_start + value_end];
        if !key.is_empty() {
            attrs.insert(key.to_owned(), xml_unescape(value));
        }
        rest = &after_eq[value_start + value_end + quote.len_utf8()..];
    }
    attrs
}

fn records_from_events(events: Vec<StationEvent>) -> Vec<StationLogRecord> {
    events
        .into_iter()
        .enumerate()
        .map(|(index, event)| StationLogRecord {
            sequence: (index + 1) as u64,
            event,
        })
        .collect()
}

fn request_id() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
        .to_string()
}

fn required_text<'a>(value: Option<&'a str>, flag: &str) -> Result<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| anyhow::anyhow!("{flag} is required"))
}

fn json_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value.get(*key)?.as_str())
}

fn json_i16(value: &Value, keys: &[&str]) -> Option<i16> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
            .and_then(|value| i16::try_from(value).ok())
    })
}

fn json_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    })
}

fn first_callsign(text: &str) -> Option<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '/' || ch == '-'))
        .filter(|token| token.len() >= 3)
        .find(|token| token.chars().any(|ch| ch.is_ascii_digit()))
        .map(|token| token.trim_matches('-').to_ascii_uppercase())
}

fn looks_like_morse(raw: &str) -> bool {
    let mut saw_mark = false;
    for ch in raw.trim().chars() {
        match ch {
            '.' | '-' => saw_mark = true,
            '/' | ' ' | '\n' | '\r' | '\t' => {}
            _ => return false,
        }
    }
    saw_mark
}

fn decode_morse_fixture(raw: &str) -> String {
    raw.split_whitespace()
        .map(|symbol| {
            if symbol == "/" {
                " ".to_owned()
            } else {
                morse_symbol(symbol).unwrap_or("?").to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("")
        .split("  ")
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn morse_symbol(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        ".-" => "A",
        "-..." => "B",
        "-.-." => "C",
        "-.." => "D",
        "." => "E",
        "..-." => "F",
        "--." => "G",
        "...." => "H",
        ".." => "I",
        ".---" => "J",
        "-.-" => "K",
        ".-.." => "L",
        "--" => "M",
        "-." => "N",
        "---" => "O",
        ".--." => "P",
        "--.-" => "Q",
        ".-." => "R",
        "..." => "S",
        "-" => "T",
        "..-" => "U",
        "...-" => "V",
        ".--" => "W",
        "-..-" => "X",
        "-.--" => "Y",
        "--.." => "Z",
        ".----" => "1",
        "..---" => "2",
        "...--" => "3",
        "....-" => "4",
        "....." => "5",
        "-...." => "6",
        "--..." => "7",
        "---.." => "8",
        "----." => "9",
        "-----" => "0",
        _ => return None,
    })
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    let compact = text
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    if compact.len() % 2 != 0 {
        bail!("hex fixture must have an even number of digits");
    }
    let mut bytes = Vec::with_capacity(compact.len() / 2);
    for pair in compact.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(pair)?;
        bytes.push(u8::from_str_radix(pair, 16).with_context(|| format!("invalid hex {pair}"))?);
    }
    Ok(bytes)
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = xml.find(&start)? + start.len();
    let end_index = xml[start_index..].find(&end)? + start_index;
    Some(xml[start_index..end_index].to_owned())
}

fn compact_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join("")
}

struct ByteReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        if self.offset + len > self.bytes.len() {
            bail!("WSJT-X datagram ended early");
        }
        let out = &self.bytes[self.offset..self.offset + len];
        self.offset += len;
        Ok(out)
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("len checked");
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_i32(&mut self) -> Result<i32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("len checked");
        Ok(i32::from_be_bytes(bytes))
    }

    fn read_f64(&mut self) -> Result<f64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("len checked");
        Ok(f64::from_be_bytes(bytes))
    }

    fn read_bool(&mut self) -> Result<bool> {
        Ok(self.take(1)?[0] != 0)
    }

    fn read_utf8(&mut self) -> Result<String> {
        let len = self.read_u32()?;
        if len == u32::MAX {
            return Ok(String::new());
        }
        let bytes = self.take(len as usize)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js8call_rx_directed_maps_to_station_event() {
        let value = json!({
            "type": "RX.DIRECTED",
            "value": "CALL HELLO",
            "params": {"FROM": "JA1QSO", "TO": "JA1TST", "SNR": -17}
        });
        let Some(StationEvent::DirectedMessage(event)) = js8call_value_to_event(&value, "JA1TST")
        else {
            panic!("expected directed message");
        };
        assert_eq!(event.from, "JA1QSO");
        assert_eq!(event.to, "JA1TST");
        assert_eq!(event.snr_db, Some(-17));
    }

    #[test]
    fn wsjtx_decode_datagram_maps_to_decode_event() {
        let datagram = wsjtx_decode_fixture("CQ JA1QSO PM95");
        let Some(StationEvent::Decode(event)) =
            parse_wsjtx_datagram(&datagram).expect("parse wsjtx")
        else {
            panic!("expected decode");
        };
        assert_eq!(event.from, Some("JA1QSO".to_owned()));
        assert_eq!(event.snr_db, Some(-12));
        assert_eq!(event.text, "FT8: CQ JA1QSO PM95");
    }

    #[test]
    fn pskreporter_xml_maps_to_spot_attrs() {
        let xml = r#"<receptionReports><receptionReport receiverCallsign="JA1TST" senderCallsign="JA1QSO" frequency="14074000" sNR="-10" /></receptionReports>"#;
        let attrs = parse_reception_report_attrs(xml);
        assert_eq!(attrs[0]["senderCallsign"], "JA1QSO");
        assert_eq!(attrs[0]["frequency"], "14074000");
    }

    #[test]
    fn morse_fixture_decodes_plain_text() {
        assert_eq!(
            decode_morse_fixture("-.-. --.- / - . ... - / .--- .- .---- --.- ... ---"),
            "CQ TEST JA1QSO"
        );
    }

    fn wsjtx_decode_fixture(message: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&WSJTX_MAGIC.to_be_bytes());
        out.extend_from_slice(&3_u32.to_be_bytes());
        out.extend_from_slice(&WSJTX_DECODE_TYPE.to_be_bytes());
        push_utf8(&mut out, "WSJT-X");
        out.push(1);
        out.extend_from_slice(&12_345_u32.to_be_bytes());
        out.extend_from_slice(&(-12_i32).to_be_bytes());
        out.extend_from_slice(&0.1_f64.to_be_bytes());
        out.extend_from_slice(&1_500_u32.to_be_bytes());
        push_utf8(&mut out, "FT8");
        push_utf8(&mut out, message);
        out.push(0);
        out.push(0);
        out
    }

    fn push_utf8(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
}
