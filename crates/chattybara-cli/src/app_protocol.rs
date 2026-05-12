use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const APP_PROTOCOL_VERSION: &str = "CBAPP/1";
pub(crate) const MAX_APP_PACKET_BYTES: usize = 512;
pub(crate) const DEFAULT_FRAGMENT_DATA_BYTES: usize = 32;
pub(crate) const DEFAULT_FILE_CHUNK_DATA_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AppPacketKind {
    Beacon,
    Cq,
    Mail,
    FileOffer,
    FileChunk,
    Fragment,
    Ack,
    Receipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AppDeliveryState {
    New,
    Sent,
    Acknowledged,
    Duplicate,
    Timeout,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppProtocolPacket {
    pub kind: AppPacketKind,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<AppDeliveryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment_total: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_hex: Option<String>,
}

impl AppProtocolPacket {
    fn new(kind: AppPacketKind, from: String, to: String) -> Self {
        Self {
            kind,
            from,
            to,
            id: None,
            sequence: None,
            timestamp_ms: None,
            delivery: None,
            ack_required: None,
            receipt_for: None,
            text: None,
            subject: None,
            body: None,
            filename: None,
            byte_count: None,
            sha256: None,
            note: None,
            message_id: None,
            file_id: None,
            fragment_index: None,
            fragment_total: None,
            chunk_sha256: None,
            data_hex: None,
        }
    }

    pub(crate) fn require_text(&self, field_name: &str) -> Result<String> {
        self.text
            .clone()
            .filter(|value| !value.is_empty())
            .with_context(|| format!("CBAPP/1 {:?} packet is missing {field_name}", self.kind))
    }

    pub(crate) fn require_subject_body(&self) -> Result<(String, String)> {
        let subject = self
            .subject
            .clone()
            .filter(|value| !value.is_empty())
            .context("CBAPP/1 mail packet is missing subject")?;
        let body = self
            .body
            .clone()
            .filter(|value| !value.is_empty())
            .context("CBAPP/1 mail packet is missing body")?;
        Ok((subject, body))
    }

    pub(crate) fn require_file_offer(&self) -> Result<(String, u64, String, Option<String>)> {
        let filename = self
            .filename
            .clone()
            .filter(|value| !value.is_empty())
            .context("CBAPP/1 file offer packet is missing filename")?;
        let byte_count = self
            .byte_count
            .context("CBAPP/1 file offer packet is missing byte_count")?;
        let sha256 = self
            .sha256
            .clone()
            .filter(|value| !value.is_empty())
            .context("CBAPP/1 file offer packet is missing sha256")?;
        Ok((filename, byte_count, sha256, self.note.clone()))
    }

    pub(crate) fn ack_for(&self) -> Option<&str> {
        self.receipt_for.as_deref()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AppProtocolState {
    station_call: String,
    next_sequence: u64,
    seen_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedAppPacket {
    pub id: Option<String>,
    pub delivery: AppDeliveryState,
    pub duplicate: bool,
}

impl AppProtocolState {
    pub(crate) fn new(station_call: &str) -> Self {
        Self {
            station_call: station_call.trim().to_ascii_uppercase(),
            next_sequence: 1,
            seen_ids: BTreeSet::new(),
        }
    }

    pub(crate) fn beacon(&mut self, to: &str, text: &str) -> AppProtocolPacket {
        let mut packet = self.base_packet(AppPacketKind::Beacon, to, true);
        packet.text = Some(text.to_owned());
        packet
    }

    pub(crate) fn cq(&mut self, to: &str, text: &str) -> AppProtocolPacket {
        let mut packet = self.base_packet(AppPacketKind::Cq, to, true);
        packet.text = Some(text.to_owned());
        packet
    }

    pub(crate) fn mail(&mut self, to: &str, subject: &str, body: &str) -> AppProtocolPacket {
        let mut packet = self.base_packet(AppPacketKind::Mail, to, true);
        packet.subject = Some(subject.to_owned());
        packet.body = Some(body.to_owned());
        packet
    }

    pub(crate) fn file_offer(
        &mut self,
        to: &str,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> AppProtocolPacket {
        let mut packet = self.base_packet(AppPacketKind::FileOffer, to, true);
        packet.filename = Some(filename.to_owned());
        packet.byte_count = Some(byte_count);
        packet.sha256 = Some(sha256.to_owned());
        packet.note = note;
        packet
    }

    pub(crate) fn ack(
        &mut self,
        to: &str,
        receipt_for: &str,
        state: AppDeliveryState,
    ) -> AppProtocolPacket {
        let mut packet = self.base_packet(AppPacketKind::Ack, to, false);
        packet.receipt_for = Some(receipt_for.to_owned());
        packet.delivery = Some(state);
        packet
    }

    pub(crate) fn fragment_payload(
        &mut self,
        to: &str,
        label: &str,
        payload: &[u8],
        max_data_bytes: usize,
    ) -> Result<Vec<AppProtocolPacket>> {
        if payload.is_empty() {
            bail!("cannot fragment an empty app payload");
        }
        let max_data_bytes = checked_chunk_size(max_data_bytes)?;
        let message_id = self.next_id();
        let total = fragment_total(payload.len(), max_data_bytes)?;
        let payload_sha256 = sha256_hex(payload);
        let mut packets = Vec::with_capacity(total as usize);
        for (index, chunk) in payload.chunks(max_data_bytes).enumerate() {
            let mut packet = self.base_packet(AppPacketKind::Fragment, to, true);
            packet.message_id = Some(message_id.clone());
            packet.text = Some(label.to_owned());
            packet.byte_count = Some(payload.len() as u64);
            packet.sha256 = Some(payload_sha256.clone());
            packet.fragment_index = Some(index as u32);
            packet.fragment_total = Some(total);
            packet.chunk_sha256 = Some(sha256_hex(chunk));
            packet.data_hex = Some(hex_encode(chunk));
            ensure_app_packet_size(&encode_app_packet_unchecked(&packet)?)?;
            packets.push(packet);
        }
        Ok(packets)
    }

    pub(crate) fn file_transfer_packets(
        &mut self,
        to: &str,
        filename: &str,
        bytes: &[u8],
        note: Option<String>,
        max_chunk_data_bytes: usize,
    ) -> Result<Vec<AppProtocolPacket>> {
        if bytes.is_empty() {
            bail!("cannot send an empty file transfer");
        }
        let max_chunk_data_bytes = checked_chunk_size(max_chunk_data_bytes)?;
        let file_sha256 = sha256_hex(bytes);
        let mut offer = self.file_offer(to, filename, bytes.len() as u64, &file_sha256, note);
        let file_id = offer
            .id
            .clone()
            .context("file offer was not assigned an app packet id")?;
        offer.file_id = Some(file_id.clone());
        let total = fragment_total(bytes.len(), max_chunk_data_bytes)?;
        let mut packets = Vec::with_capacity(total as usize + 1);
        ensure_app_packet_size(&encode_app_packet_unchecked(&offer)?)?;
        packets.push(offer);
        for (index, chunk) in bytes.chunks(max_chunk_data_bytes).enumerate() {
            let mut packet = self.base_packet(AppPacketKind::FileChunk, to, true);
            packet.file_id = Some(file_id.clone());
            packet.filename = Some(filename.to_owned());
            packet.byte_count = Some(bytes.len() as u64);
            packet.sha256 = Some(file_sha256.clone());
            packet.fragment_index = Some(index as u32);
            packet.fragment_total = Some(total);
            packet.chunk_sha256 = Some(sha256_hex(chunk));
            packet.data_hex = Some(hex_encode(chunk));
            ensure_app_packet_size(&encode_app_packet_unchecked(&packet)?)?;
            packets.push(packet);
        }
        Ok(packets)
    }

    pub(crate) fn observe(&mut self, packet: &AppProtocolPacket) -> ObservedAppPacket {
        let Some(id) = packet.id.clone() else {
            return ObservedAppPacket {
                id: None,
                delivery: AppDeliveryState::New,
                duplicate: false,
            };
        };
        let duplicate = !self.seen_ids.insert(id.clone());
        ObservedAppPacket {
            id: Some(id),
            delivery: if duplicate {
                AppDeliveryState::Duplicate
            } else {
                AppDeliveryState::Acknowledged
            },
            duplicate,
        }
    }

    fn base_packet(
        &mut self,
        kind: AppPacketKind,
        to: &str,
        ack_required: bool,
    ) -> AppProtocolPacket {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let mut packet = AppProtocolPacket::new(
            kind,
            self.station_call.clone(),
            to.trim().to_ascii_uppercase(),
        );
        packet.id = Some(format!("{}-{sequence:08}", self.station_call));
        packet.sequence = Some(sequence);
        packet.timestamp_ms = Some(sequence);
        packet.delivery = Some(AppDeliveryState::Sent);
        packet.ack_required = Some(ack_required);
        packet
    }

    fn next_id(&mut self) -> String {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        format!("{}-{sequence:08}", self.station_call)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReassembledPayload {
    pub message_id: String,
    pub label: Option<String>,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReassembledFile {
    pub file_id: String,
    pub filename: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

pub(crate) fn encode_app_packet(packet: &AppProtocolPacket) -> Result<String> {
    let payload = encode_app_packet_unchecked(packet)?;
    ensure_app_packet_size(&payload)?;
    Ok(payload)
}

fn encode_app_packet_unchecked(packet: &AppProtocolPacket) -> Result<String> {
    let json = serde_json::to_string(packet).context("serializing CBAPP/1 app packet")?;
    Ok(format!("{APP_PROTOCOL_VERSION}\n{json}"))
}

pub(crate) fn decode_app_packet(payload: &str) -> Result<Option<AppProtocolPacket>> {
    let Some(rest) = payload.strip_prefix(APP_PROTOCOL_VERSION) else {
        if payload.starts_with("CBAPP/") {
            bail!("unsupported CBAPP app envelope version; expected CBAPP/1");
        }
        return Ok(None);
    };
    let Some(json) = rest.strip_prefix('\n') else {
        bail!("malformed CBAPP/1 app envelope; expected newline after version");
    };
    if json.trim().is_empty() {
        bail!("malformed CBAPP/1 app envelope; missing JSON body");
    }
    let packet = serde_json::from_str(json).context("decoding CBAPP/1 app packet")?;
    Ok(Some(packet))
}

pub(crate) fn ensure_app_packet_size(payload: &str) -> Result<()> {
    let len = payload.len();
    if len > MAX_APP_PACKET_BYTES {
        bail!(
            "app packet is {len} bytes; maximum one-packet payload is {MAX_APP_PACKET_BYTES} bytes"
        );
    }
    Ok(())
}

pub(crate) fn reassemble_fragments(packets: &[AppProtocolPacket]) -> Result<ReassembledPayload> {
    if packets.is_empty() {
        bail!("cannot reassemble zero app fragments");
    }
    let first = &packets[0];
    if first.kind != AppPacketKind::Fragment {
        bail!("cannot reassemble non-fragment app packet");
    }
    let message_id = required_string(first.message_id.as_deref(), "fragment message_id")?;
    let expected_total = first
        .fragment_total
        .context("fragment is missing fragment_total")?;
    let expected_sha256 = required_string(first.sha256.as_deref(), "fragment sha256")?;
    let expected_byte_count = first.byte_count.context("fragment is missing byte_count")?;
    let label = first.text.clone();
    let mut by_index = BTreeMap::new();
    for packet in packets {
        if packet.kind != AppPacketKind::Fragment {
            bail!("mixed non-fragment packet in fragment reassembly");
        }
        validate_fragment_identity(packet, &message_id, expected_total, &expected_sha256)?;
        let index = packet
            .fragment_index
            .context("fragment is missing fragment_index")?;
        if by_index.insert(index, packet).is_some() {
            bail!("duplicate fragment index {index} for message {message_id}");
        }
    }
    if by_index.len() != expected_total as usize {
        bail!(
            "fragment message {message_id} has {} of {expected_total} fragments",
            by_index.len()
        );
    }
    let mut bytes = Vec::new();
    for index in 0..expected_total {
        let packet = by_index
            .get(&index)
            .with_context(|| format!("missing fragment index {index} for message {message_id}"))?;
        let data_hex = required_string(packet.data_hex.as_deref(), "fragment data_hex")?;
        let data = hex_decode(&data_hex)?;
        let chunk_sha256 =
            required_string(packet.chunk_sha256.as_deref(), "fragment chunk_sha256")?;
        if sha256_hex(&data) != chunk_sha256 {
            bail!("fragment {index} for message {message_id} failed chunk hash validation");
        }
        bytes.extend(data);
    }
    if bytes.len() as u64 != expected_byte_count {
        bail!(
            "fragment message {message_id} reassembled to {} bytes, expected {expected_byte_count}",
            bytes.len()
        );
    }
    if sha256_hex(&bytes) != expected_sha256 {
        bail!("fragment message {message_id} failed payload hash validation");
    }
    Ok(ReassembledPayload {
        message_id,
        label,
        bytes,
        sha256: expected_sha256,
    })
}

pub(crate) fn reassemble_file_chunks(packets: &[AppProtocolPacket]) -> Result<ReassembledFile> {
    if packets.is_empty() {
        bail!("cannot reassemble zero file chunks");
    }
    let chunks: Vec<&AppProtocolPacket> = packets
        .iter()
        .filter(|packet| packet.kind == AppPacketKind::FileChunk)
        .collect();
    if chunks.is_empty() {
        bail!("file transfer does not contain file chunks");
    }
    let first = chunks[0];
    let file_id = required_string(first.file_id.as_deref(), "file_id")?;
    let filename = required_string(first.filename.as_deref(), "filename")?;
    let expected_total = first
        .fragment_total
        .context("file chunk is missing fragment_total")?;
    let expected_sha256 = required_string(first.sha256.as_deref(), "file sha256")?;
    let expected_byte_count = first
        .byte_count
        .context("file chunk is missing byte_count")?;
    let mut by_index = BTreeMap::new();
    for packet in chunks {
        if required_string(packet.file_id.as_deref(), "file_id")? != file_id {
            bail!("mixed file_id values in file reassembly");
        }
        if required_string(packet.filename.as_deref(), "filename")? != filename {
            bail!("mixed filenames in file reassembly");
        }
        if required_string(packet.sha256.as_deref(), "file sha256")? != expected_sha256 {
            bail!("mixed file sha256 values in file reassembly");
        }
        if packet.fragment_total != Some(expected_total) {
            bail!("mixed fragment_total values in file reassembly");
        }
        let index = packet
            .fragment_index
            .context("file chunk is missing fragment_index")?;
        if by_index.insert(index, packet).is_some() {
            bail!("duplicate file chunk index {index} for file {file_id}");
        }
    }
    if by_index.len() != expected_total as usize {
        bail!(
            "file {file_id} has {} of {expected_total} chunks",
            by_index.len()
        );
    }
    let mut bytes = Vec::new();
    for index in 0..expected_total {
        let packet = by_index
            .get(&index)
            .with_context(|| format!("missing file chunk index {index} for file {file_id}"))?;
        let data_hex = required_string(packet.data_hex.as_deref(), "file chunk data_hex")?;
        let data = hex_decode(&data_hex)?;
        let chunk_sha256 =
            required_string(packet.chunk_sha256.as_deref(), "file chunk chunk_sha256")?;
        if sha256_hex(&data) != chunk_sha256 {
            bail!("file chunk {index} for {file_id} failed chunk hash validation");
        }
        bytes.extend(data);
    }
    if bytes.len() as u64 != expected_byte_count {
        bail!(
            "file {file_id} reassembled to {} bytes, expected {expected_byte_count}",
            bytes.len()
        );
    }
    if sha256_hex(&bytes) != expected_sha256 {
        bail!("file {file_id} failed full-file hash validation");
    }
    Ok(ReassembledFile {
        file_id,
        filename,
        bytes,
        sha256: expected_sha256,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct SimulatedAppLinkConfig {
    pub max_retries: usize,
    pub timeout_ticks: u64,
    pub drop_first_attempt: bool,
    pub drop_all_attempts: bool,
    pub duplicate_deliveries: bool,
}

impl Default for SimulatedAppLinkConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            timeout_ticks: 3,
            drop_first_attempt: false,
            drop_all_attempts: false,
            duplicate_deliveries: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SimulatedAppLinkReport {
    pub delivered: usize,
    pub acknowledged: usize,
    pub attempts: usize,
    pub timeouts: usize,
    pub duplicate_packets: usize,
    pub failed_ids: Vec<String>,
}

pub(crate) fn simulate_reliable_delivery(
    packets: &[AppProtocolPacket],
    config: SimulatedAppLinkConfig,
) -> Result<SimulatedAppLinkReport> {
    let mut receiver = AppProtocolState::new("RX");
    let mut report = SimulatedAppLinkReport {
        delivered: 0,
        acknowledged: 0,
        attempts: 0,
        timeouts: 0,
        duplicate_packets: 0,
        failed_ids: Vec::new(),
    };

    for packet in packets {
        let id = packet.id.clone().unwrap_or_else(|| "<no-id>".to_owned());
        let mut delivered = false;
        for attempt in 0..=config.max_retries {
            report.attempts += 1;
            if config.drop_all_attempts || (attempt == 0 && config.drop_first_attempt) {
                report.timeouts += config.timeout_ticks.max(1) as usize;
                continue;
            }

            let observed = receiver.observe(packet);
            if observed.duplicate {
                report.duplicate_packets += 1;
            } else {
                report.delivered += 1;
                report.acknowledged += 1;
            }
            if config.duplicate_deliveries {
                let duplicate = receiver.observe(packet);
                if duplicate.duplicate {
                    report.duplicate_packets += 1;
                }
            }
            delivered = true;
            break;
        }
        if !delivered {
            report.failed_ids.push(id);
        }
    }
    Ok(report)
}

fn validate_fragment_identity(
    packet: &AppProtocolPacket,
    message_id: &str,
    expected_total: u32,
    expected_sha256: &str,
) -> Result<()> {
    if required_string(packet.message_id.as_deref(), "fragment message_id")? != message_id {
        bail!("mixed message_id values in fragment reassembly");
    }
    if packet.fragment_total != Some(expected_total) {
        bail!("mixed fragment_total values in fragment reassembly");
    }
    if required_string(packet.sha256.as_deref(), "fragment sha256")? != expected_sha256 {
        bail!("mixed payload sha256 values in fragment reassembly");
    }
    Ok(())
}

fn checked_chunk_size(size: usize) -> Result<usize> {
    if size == 0 {
        bail!("chunk size must be greater than zero");
    }
    Ok(size)
}

fn fragment_total(byte_count: usize, chunk_size: usize) -> Result<u32> {
    let total = byte_count.div_ceil(chunk_size);
    u32::try_from(total).context("fragment count exceeds u32 range")
}

fn required_string(value: Option<&str>, field_name: &str) -> Result<String> {
    value
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("missing {field_name}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str) -> Result<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        bail!("hex payload has odd length");
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid hex byte {:?}", byte as char),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_packets_have_reliability_metadata() {
        let mut protocol = AppProtocolState::new("ja1tst");
        let packet = protocol.mail("ja1qso", "Subject", "Body");
        let payload = encode_app_packet(&packet).expect("encode packet");

        assert!(payload.starts_with("CBAPP/1\n"));
        assert!(payload.contains("\"kind\":\"mail\""));
        assert!(payload.contains("\"id\":\"JA1TST-00000001\""));
        assert!(payload.contains("\"sequence\":1"));
        assert!(payload.contains("\"delivery\":\"sent\""));
        assert!(payload.contains("\"ack_required\":true"));

        let decoded = decode_app_packet(&payload)
            .expect("decode packet")
            .expect("app packet");
        assert_eq!(decoded, packet);
    }

    #[test]
    fn app_fragments_reassemble_and_validate_hashes() {
        let mut protocol = AppProtocolState::new("ja1tst");
        let bytes =
            b"packet fragmentation needs enough bytes to split cleanly over multiple frames";
        let fragments = protocol
            .fragment_payload("ja1qso", "mail", bytes, 16)
            .expect("fragment payload");

        assert!(fragments.len() > 1);
        for fragment in &fragments {
            let payload = encode_app_packet(fragment).expect("encode fragment");
            assert!(payload.len() <= MAX_APP_PACKET_BYTES);
        }

        let reassembled = reassemble_fragments(&fragments).expect("reassemble fragments");
        assert_eq!(reassembled.label.as_deref(), Some("mail"));
        assert_eq!(reassembled.bytes, bytes);
    }

    #[test]
    fn file_transfer_packets_reassemble_file_bytes() {
        let mut protocol = AppProtocolState::new("ja1tst");
        let bytes = b"this is the file content that will be split into chunks and verified";
        let packets = protocol
            .file_transfer_packets(
                "ja1qso",
                "sample.txt",
                bytes,
                Some("test note".to_owned()),
                12,
            )
            .expect("build file transfer");

        assert_eq!(packets[0].kind, AppPacketKind::FileOffer);
        assert!(
            packets
                .iter()
                .any(|packet| packet.kind == AppPacketKind::FileChunk)
        );
        for packet in &packets {
            let payload = encode_app_packet(packet).expect("encode file packet");
            assert!(payload.len() <= MAX_APP_PACKET_BYTES);
        }

        let reassembled = reassemble_file_chunks(&packets).expect("reassemble file");
        assert_eq!(reassembled.filename, "sample.txt");
        assert_eq!(reassembled.bytes, bytes);
    }

    #[test]
    fn reliable_delivery_simulator_retries_and_detects_duplicates() {
        let mut protocol = AppProtocolState::new("ja1tst");
        let packets = vec![protocol.beacon("ja1qso", "monitoring")];
        let report = simulate_reliable_delivery(
            &packets,
            SimulatedAppLinkConfig {
                max_retries: 2,
                timeout_ticks: 1,
                drop_first_attempt: true,
                drop_all_attempts: false,
                duplicate_deliveries: true,
            },
        )
        .expect("simulate delivery");

        assert_eq!(report.failed_ids, Vec::<String>::new());
        assert_eq!(report.delivered, 1);
        assert_eq!(report.acknowledged, 1);
        assert_eq!(report.attempts, 2);
        assert_eq!(report.timeouts, 1);
        assert_eq!(report.duplicate_packets, 1);
    }

    #[test]
    fn reliable_delivery_simulator_reports_failed_timeouts() {
        let mut protocol = AppProtocolState::new("ja1tst");
        let packets = vec![protocol.beacon("ja1qso", "monitoring")];
        let report = simulate_reliable_delivery(
            &packets,
            SimulatedAppLinkConfig {
                max_retries: 2,
                timeout_ticks: 2,
                drop_first_attempt: false,
                drop_all_attempts: true,
                duplicate_deliveries: false,
            },
        )
        .expect("simulate delivery");

        assert_eq!(report.delivered, 0);
        assert_eq!(report.acknowledged, 0);
        assert_eq!(report.attempts, 3);
        assert_eq!(report.timeouts, 6);
        assert_eq!(report.failed_ids, vec!["JA1TST-00000001"]);
    }
}
