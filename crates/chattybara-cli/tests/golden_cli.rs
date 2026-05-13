use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn chattybara() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chattybara"));
    command.env(
        "CHATTYBARA_SETTINGS",
        "target/test-no-local-chattybara-settings.toml",
    );
    command
}

fn run_json(args: &[&str]) -> Value {
    let output = chattybara().args(args).output().expect("run chattybara");
    json_from_success(output, &format!("{args:?}"))
}

fn json_from_success(output: Output, context: &str) -> Value {
    assert!(
        output.status.success(),
        "command failed\ncontext: {context}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("write hex");
    }
    out
}

fn run_text(args: &[&str]) -> String {
    let output = chattybara().args(args).output().expect("run chattybara");
    assert!(
        output.status.success(),
        "command failed\nargs: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

fn run_text_failure(args: &[&str]) -> String {
    let output = chattybara().args(args).output().expect("run chattybara");
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nargs: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_json_failure(args: &[&str]) -> Value {
    let output = chattybara().args(args).output().expect("run chattybara");
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nargs: {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json stdout")
}

#[test]
fn synthetic_audio_to_pipeline_flow_is_stable() {
    let dir = tempdir().expect("tempdir");
    let tone = dir.path().join("tone.wav");
    let loopback = dir.path().join("tone-loopback.wav");
    let tone_arg = path_arg(&tone);
    let loopback_arg = path_arg(&loopback);

    let synth = run_json(&[
        "fixture",
        "synth",
        &tone_arg,
        "--kind",
        "tone-burst",
        "--frequency",
        "1000",
    ]);
    assert_eq!(synth["sample_rate"], 8000);
    assert_eq!(
        synth["energy_regions"].as_array().expect("regions").len(),
        1
    );

    let chunks = run_json(&["audio", "chunks", "--frames", "256", &tone_arg]);
    assert_eq!(chunks["chunk_count"], 32);
    assert_eq!(chunks["total_frames"], 8000);

    let loopback_report = run_json(&[
        "audio",
        "loopback",
        "--latency-frames",
        "80",
        "--gain",
        "0.8",
        &tone_arg,
        &loopback_arg,
    ]);
    assert_eq!(loopback_report["input_frames"], 8000);
    assert_eq!(loopback_report["output_frames"], 8080);

    let classification = run_json(&["frames", "classify", &loopback_arg]);
    assert_eq!(classification["candidate_count"], 1);
    assert_eq!(classification["candidates"][0]["class"], "narrowband-burst");

    let pipeline = run_json(&["frames", "pipeline", &loopback_arg]);
    assert_eq!(pipeline["kind"], "receive-pipeline-report");
    assert_eq!(pipeline["classifications"]["candidate_count"], 1);
    assert!(
        !pipeline["link_events"]
            .as_array()
            .expect("events")
            .is_empty()
    );
}

#[test]
fn host_script_is_stateful() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("host-script.txt");
    fs::write(
        &script,
        "MYCALL ja1tst\nCONNECT ja1qso\nCONNECTED\nSEND hello\nACK 1\nDISCONNECT\nPEER-DISCONNECTED\n",
    )
    .expect("write script");

    let replies = run_json(&["host", "script", &path_arg(&script)]);

    assert_eq!(replies.as_array().expect("replies").len(), 7);
    assert_eq!(replies[0]["reply"]["actions"][0]["state"], "idle");
    assert_eq!(replies[3]["reply"]["actions"][0]["kind"], "send-payload");
    assert_eq!(replies[3]["reply"]["actions"][0]["sequence"], 1);
    assert_eq!(replies[6]["reply"]["actions"][0]["state"], "idle");
}

#[test]
fn chat_fake_script_runs_basic_qso_without_modem() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("chat-script.txt");
    fs::write(
        &script,
        "CONNECT ja1qso\nSEND hello from test\nRX ja1qso roger\nSTATUS\nDISCONNECT\n",
    )
    .expect("write script");

    let report = run_json(&[
        "chat",
        "fake-script",
        &path_arg(&script),
        "--station",
        "ja1tst",
    ]);

    assert_eq!(report["kind"], "chat-script-report");
    assert_eq!(report["backend"], "fake");
    assert_eq!(report["ok"], true);
    assert_eq!(report["transcript"]["station"]["call_sign"], "JA1TST");
    assert_eq!(
        report["transcript"]["messages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(report["transcript"]["messages"][0]["direction"], "outbound");
    assert_eq!(report["transcript"]["messages"][1]["direction"], "inbound");
    assert_eq!(report["commands"][0]["event"]["kind"], "connected");
    assert_eq!(report["commands"][3]["event"]["kind"], "status");
    assert_eq!(report["commands"][4]["event"]["kind"], "disconnected");
}

#[test]
fn audio_devices_reports_inventory() {
    let report = run_json(&[
        "audio",
        "devices",
        "--sample-rate",
        "8000",
        "--channels",
        "1",
    ]);

    assert_eq!(report["kind"], "audio-device-inventory");
    assert_eq!(report["requested_config"]["sample_rate"], 8000);
    assert_eq!(report["requested_config"]["channels"], 1);
    assert!(!report["host"].as_str().expect("host").is_empty());
    assert!(report["devices"].as_array().is_some());
}

#[test]
fn chat_app_script_reports_clean_room_features() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("app-script.txt");
    fs::write(
        &script,
        concat!(
            "BEACON monitoring 14.105 USB\n",
            "CQ testing local app model\n",
            "MAIL ja1qso Test subject | Synthetic mailbox body\n",
            "FILE-OFFER ja1qso sample.txt 42 ",
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855 ",
            "metadata only\n",
            "STATUS\n",
        ),
    )
    .expect("write script");

    let report = run_json(&[
        "chat",
        "app-script",
        &path_arg(&script),
        "--station",
        "ja1tst",
    ]);

    assert_eq!(report["kind"], "chat-app-script-report");
    assert_eq!(report["backend"], "native-app-model");
    assert_eq!(report["ok"], true);
    assert_eq!(report["station"]["call_sign"], "JA1TST");
    assert_eq!(report["state"]["beacons"].as_array().unwrap().len(), 1);
    assert_eq!(report["state"]["cq_calls"].as_array().unwrap().len(), 1);
    assert_eq!(report["state"]["mailbox"][0]["to"], "JA1QSO");
    assert_eq!(report["state"]["mailbox"][0]["subject"], "Test subject");
    assert_eq!(report["state"]["file_offers"][0]["filename"], "sample.txt");
    assert_eq!(
        report["state"]["file_offers"][0]["sha256"],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(report["commands"][4]["event"]["kind"], "status");
}

#[test]
fn chat_tui_help_exposes_fake_backend() {
    let help = run_text(&["chat", "tui", "--help"]);

    assert!(help.contains("--station"));
    assert!(help.contains("chattybara station config --station CALL"));
    assert!(help.contains("--backend"));
    assert!(help.contains("--setup-preview"));
    assert!(help.contains("fake"));
    assert!(help.contains("native-loopback"));
    assert!(help.contains("native-wav-loopback"));
    assert!(help.contains("native-local-node"));
    assert!(help.contains("--peer"));
    assert!(help.contains("--listen"));
    assert!(help.contains("--connect"));
}

#[test]
fn chat_tui_setup_preview_works_without_required_flags() {
    let preview = run_json(&["chat", "tui", "--setup-preview"]);

    assert_eq!(preview["kind"], "chat-tui-setup-preview");
    assert_eq!(preview["command"], "chattybara chat tui");
    assert_eq!(preview["product"], "chattybara");
    assert_eq!(preview["modem_engine"], "orca");
    assert_eq!(preview["station"], "JA1TST");
    assert_eq!(preview["starts_in_setup"], true);
    assert_eq!(preview["running_backend"], "native-loopback");
    assert_eq!(preview["selected_backend"], "native-loopback");
    assert!(
        preview["setup_commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|value| value == "/station CALL")
    );

    let preview = run_json(&[
        "chat",
        "tui",
        "--setup-preview",
        "--station",
        "ja1tst",
        "--peer",
        "ja1qso",
    ]);

    assert_eq!(preview["station"], "JA1TST");
    assert_eq!(preview["peer"], "JA1QSO");
}

#[test]
fn local_station_settings_feed_tui_and_winlink_defaults() {
    let dir = tempdir().expect("tempdir");
    let settings = dir.path().join("settings.toml");
    let settings_arg = path_arg(&settings);

    let config = run_json(&[
        "station",
        "config",
        "--station",
        "ja1abc",
        "--path",
        &settings_arg,
    ]);
    assert_eq!(config["kind"], "station-local-settings-report");
    assert_eq!(config["settings"]["station"], "JA1ABC");
    assert!(settings.exists());

    let preview = json_from_success(
        chattybara()
            .args(["chat", "tui", "--setup-preview"])
            .env("CHATTYBARA_SETTINGS", &settings)
            .output()
            .expect("run chattybara"),
        "chat tui setup preview with local station settings",
    );
    assert_eq!(preview["station"], "JA1ABC");

    let telnet = json_from_success(
        chattybara()
            .args(["winlink", "telnet", "--check"])
            .env("CHATTYBARA_SETTINGS", &settings)
            .output()
            .expect("run chattybara"),
        "winlink telnet with local station settings",
    );
    assert_eq!(telnet["transport_status"]["station"], "JA1ABC");
}

#[test]
fn chat_tui_backend_typo_reports_no_space_hint() {
    let error = run_text_failure(&[
        "chat",
        "tui",
        "--station",
        "JA1TST",
        "--backend",
        "native-",
        "wav-loopback",
    ]);

    assert!(error.contains("remove the space"));
    assert!(error.contains("native-wav-loopback"));
}

#[test]
fn winlink_fake_sync_exercises_mail_store_without_radio() {
    let dir = tempdir().expect("tempdir");
    let store = dir.path().join("winlink-store.json");
    let store_arg = path_arg(&store);

    let account = run_json(&[
        "winlink",
        "account",
        "setup",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
        "--password-source",
        "env",
    ]);
    assert_eq!(account["kind"], "winlink-account-setup-report");
    assert_eq!(account["station"], "JA1TST");
    assert_eq!(account["address"], "JA1TST@winlink.org");
    assert_eq!(account["password_source"], "env");

    let compose = run_json(&[
        "winlink",
        "compose",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
        "--to",
        "ja1qso",
        "--subject",
        "No radio test",
        "--body",
        "Testing Winlink fake sync",
    ]);
    assert_eq!(compose["kind"], "winlink-compose-report");
    assert_eq!(compose["station"], "JA1TST");
    assert_eq!(compose["folder"], "outbox");
    assert_eq!(compose["b2f_proposal"]["subject"], "No radio test");
    let message_id = compose["message_id"].as_str().expect("message id");

    let outbox = run_json(&[
        "winlink",
        "outbox",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
    ]);
    assert_eq!(outbox["kind"], "winlink-mailbox-report");
    assert_eq!(outbox["message_count"], 1);
    assert_eq!(outbox["messages"][0]["id"], message_id);

    let sync = run_json(&[
        "winlink",
        "sync",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
        "--transport",
        "fake",
    ]);
    assert_eq!(sync["kind"], "winlink-sync-report");
    assert_eq!(sync["transport"], "fake");
    assert_eq!(sync["inbox_received"], 1);
    assert_eq!(sync["outbox_sent"], 1);
    assert_eq!(sync["queued_remaining"], 0);

    let inbox = run_json(&[
        "winlink",
        "inbox",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
    ]);
    assert_eq!(inbox["message_count"], 1);
    let inbox_id = inbox["messages"][0]["id"].as_str().expect("inbox id");

    let message = run_json(&[
        "winlink",
        "read",
        inbox_id,
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
    ]);
    assert_eq!(message["kind"], "winlink-message-report");
    assert_eq!(message["message"]["folder"], "inbox");
    assert_eq!(message["message"]["to"][0], "JA1TST@winlink.org");
}

#[test]
fn winlink_transport_surfaces_are_guarded() {
    let telnet = run_json(&["winlink", "telnet", "--station", "ja1tst", "--check"]);
    assert_eq!(telnet["kind"], "winlink-telnet-check-report");
    assert_eq!(telnet["transport_status"]["transport"], "telnet-cms");
    assert_eq!(telnet["transport_status"]["dry_run"], true);
    assert_eq!(telnet["transport_status"]["connected"], false);

    let vara = run_json(&[
        "winlink",
        "transport",
        "--station",
        "ja1tst",
        "--transport",
        "vara",
    ]);
    assert_eq!(vara["transport"], "vara");
    assert_eq!(vara["dry_run"], true);
    assert!(
        vara["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().unwrap_or_default().contains("external"))
    );

    let orca = run_json(&[
        "winlink",
        "transport",
        "--station",
        "ja1tst",
        "--transport",
        "orca",
    ]);
    assert_eq!(orca["transport"], "orca");
    assert!(
        orca["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .any(|note| note.as_str().unwrap_or_default().contains("open modem"))
    );
}

#[test]
fn winlink_telnet_live_sync_lists_fake_cms_inbox() {
    let dir = tempdir().expect("tempdir");
    let store = dir.path().join("winlink-store.json");
    let store_arg = path_arg(&store);
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));

        stream.write_all(b"Callsign :\r").expect("write callsign");
        assert_eq!(read_nonempty_cr_line(&mut reader), "JA1TST");
        stream.write_all(b"Password :\r").expect("write password");
        assert_eq!(read_nonempty_cr_line(&mut reader), "CMSTELNET");
        stream
            .write_all(b"[WL2K-5.0-B2FHM$]\r;PQ: 23753528\rCMS>\r")
            .expect("write handshake");
        assert_eq!(read_nonempty_cr_line(&mut reader), ";FW: JA1TST");
        assert!(read_nonempty_cr_line(&mut reader).starts_with("[chattybara-"));
        assert_eq!(read_nonempty_cr_line(&mut reader), ";PR: 95074758");
        assert_eq!(read_nonempty_cr_line(&mut reader), "; CMS DE JA1TST>");
        assert_eq!(read_nonempty_cr_line(&mut reader), "FF");

        let body = b"CLI downloaded body.";
        let payload = b2_fixture_message("CLI-MID-1", "JA1QSO", "JA1TST", "CLI subject", body);
        let compressed = b2_lzhuf_fixture_payload(&payload);
        let transfer = b2_transfer_fixture("CLI subject", &compressed);
        let proposal = format!("FC EM CLI-MID-1 {} {} 0", payload.len(), compressed.len());
        let checksum = b2f_checksum(std::slice::from_ref(&proposal));
        stream
            .write_all(
                format!(
                    ";PM: JA1TST CLI-MID-1 {} JA1QSO CLI subject\r{proposal}\rF> {checksum:02X}\r",
                    payload.len()
                )
                .as_bytes(),
            )
            .expect("write proposals");
        assert_eq!(read_nonempty_cr_line(&mut reader), "FS +");
        stream.write_all(&transfer).expect("write transfer");
        assert_eq!(read_nonempty_cr_line(&mut reader), "FQ");
    });

    let output = chattybara()
        .args([
            "winlink",
            "sync",
            "--station",
            "ja1tst",
            "--store",
            &store_arg,
            "--transport",
            "telnet",
            "--live",
            "--host",
            "127.0.0.1",
            "--port",
            &address.port().to_string(),
            "--timeout-ms",
            "5000",
        ])
        .env("CHATTYBARA_WINLINK_PASSWORD", "FooBar")
        .output()
        .expect("run chattybara");
    handle.join().expect("join");
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let sync: Value = serde_json::from_slice(&output.stdout).expect("sync json");
    assert_eq!(sync["kind"], "winlink-sync-report");
    assert_eq!(sync["transport"], "telnet-cms");
    assert_eq!(sync["live"], true);
    assert_eq!(sync["inbox_received"], 1);
    assert_eq!(sync["outbox_sent"], 0);

    let inbox = run_json(&[
        "winlink",
        "inbox",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
    ]);
    assert_eq!(inbox["message_count"], 1);
    assert_eq!(inbox["messages"][0]["id"], "CLI-MID-1");
    assert_eq!(inbox["messages"][0]["subject"], "CLI subject");

    let message = run_json(&[
        "winlink",
        "read",
        "CLI-MID-1",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
    ]);
    assert_eq!(message["message"]["from"], "JA1QSO@winlink.org");
    assert_eq!(message["message"]["body"], "CLI downloaded body.");
    assert_eq!(message["message"]["last_error"], Value::Null);
}

#[test]
fn winlink_live_non_fake_send_requires_explicit_allow_send() {
    let dir = tempdir().expect("tempdir");
    let store = dir.path().join("winlink-store.json");
    let store_arg = path_arg(&store);

    run_json(&[
        "winlink",
        "compose",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
        "--to",
        "ja1qso",
        "--subject",
        "Guarded",
        "--body",
        "Body",
    ]);

    let error = run_text_failure(&[
        "winlink",
        "sync",
        "--station",
        "ja1tst",
        "--store",
        &store_arg,
        "--transport",
        "vara",
        "--live",
    ]);
    assert!(error.contains("requires --allow-send"));
}

#[test]
fn station_modes_lists_multi_mode_registry() {
    let report = run_json(&["station", "modes"]);

    assert_eq!(report["kind"], "station-mode-registry");
    assert_eq!(report["ok"], true);
    assert_eq!(report["mode_count"], 9);
    assert!(
        report["modes"]
            .as_array()
            .expect("modes")
            .iter()
            .any(|mode| mode["label"] == "orca-chat"
                && mode["capabilities"]["file_transfer"] == true)
    );
    assert!(
        report["modes"]
            .as_array()
            .expect("modes")
            .iter()
            .any(|mode| mode["label"] == "wsjtx-external" && mode["workspace"] == "weak-signal")
    );
    assert!(
        report["modes"]
            .as_array()
            .expect("modes")
            .iter()
            .any(|mode| mode["label"] == "winlink-telnet"
                && mode["workspace"] == "winlink"
                && mode["capabilities"]["mailbox"] == true)
    );
}

#[test]
fn station_fake_events_write_replayable_session() {
    let dir = tempdir().expect("tempdir");
    let events = dir.path().join("events.jsonl");
    let session_dir = dir.path().join("session");

    let report = run_json(&[
        "station",
        "fake-events",
        "--mode",
        "js8call",
        "--station",
        "ja1tst",
        "--out",
        &path_arg(&events),
        "--session-dir",
        &path_arg(&session_dir),
    ]);

    assert_eq!(report["kind"], "station-fake-events-report");
    assert_eq!(report["mode"], "js8call-external");
    assert_eq!(report["summary"]["record_count"], 2);
    assert!(events.exists());
    assert!(session_dir.join("events.jsonl").exists());
    assert!(session_dir.join("support.json").exists());

    let replay = run_json(&["station", "replay", &path_arg(&events)]);
    assert_eq!(replay["kind"], "station-replay-report");
    assert_eq!(replay["summary"]["modes"]["js8call-external"], 2);
    assert_eq!(replay["summary"]["event_counts"]["directed-message"], 1);
}

#[test]
fn station_protocol_suite_covers_external_and_planned_modes() {
    let dir = tempdir().expect("tempdir");
    let out_dir = dir.path().join("protocol-suite");

    let report = run_json(&[
        "station",
        "protocol-suite",
        "--station",
        "ja1tst",
        "--out-dir",
        &path_arg(&out_dir),
    ]);

    assert_eq!(report["kind"], "station-protocol-suite-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["mode_count"], 7);
    let modes = report["modes"].as_array().expect("modes");
    for mode in [
        "js8call-external",
        "wsjtx-external",
        "fldigi-external",
        "cw-assist",
        "pskreporter",
        "winlink-vara",
        "winlink-orca",
    ] {
        assert!(
            modes.iter().any(|entry| entry["mode"] == mode),
            "missing {mode}"
        );
        assert!(out_dir.join(mode).join("events.jsonl").exists());
        assert!(out_dir.join(mode).join("support.json").exists());
    }

    let by_mode = |mode: &str| {
        modes
            .iter()
            .find(|entry| entry["mode"] == mode)
            .unwrap_or_else(|| panic!("missing mode {mode}"))
    };
    let js8call = by_mode("js8call-external");
    assert_eq!(js8call["adapter"]["endpoint"]["port"], 2442);
    assert_eq!(js8call["adapter"]["protocol"]["kind"], "tcp-json-lines");

    let wsjtx = by_mode("wsjtx-external");
    assert_eq!(wsjtx["adapter"]["endpoint"]["port"], 2237);
    assert_eq!(wsjtx["adapter"]["protocol"]["kind"], "udp-datagrams");

    let fldigi = by_mode("fldigi-external");
    assert_eq!(fldigi["adapter"]["endpoint"]["port"], 7362);
    assert_eq!(fldigi["adapter"]["protocol"]["kind"], "xml-rpc-http");

    let cw = by_mode("cw-assist");
    assert!(cw["adapter"].is_null());
    assert_eq!(cw["descriptor"]["capabilities"]["rx_only"], true);

    let psk = by_mode("pskreporter");
    assert_eq!(psk["adapter"]["protocol"]["kind"], "https-query");
    assert_eq!(psk["summary"]["event_counts"]["spot"], 1);

    let vara = by_mode("winlink-vara");
    assert_eq!(vara["transport"]["transport"], "vara");
    assert_eq!(vara["transport"]["dry_run"], true);

    let orca = by_mode("winlink-orca");
    assert_eq!(orca["transport"]["transport"], "orca");
    assert_eq!(orca["transport"]["dry_run"], true);

    let replay = run_json(&[
        "station",
        "replay",
        &path_arg(&out_dir.join("pskreporter").join("events.jsonl")),
    ]);
    assert_eq!(replay["summary"]["modes"]["pskreporter"], 2);
}

#[test]
fn station_guard_blocks_unarmed_transmit_and_reporting() {
    let send = run_json_failure(&["station", "guard", "--action", "send-message"]);
    assert_eq!(send["kind"], "station-action-guard-report");
    assert_eq!(send["ok"], false);
    assert!(
        send["error"]
            .as_str()
            .expect("error")
            .contains("requires TX")
    );

    let report = run_json_failure(&["station", "guard", "--action", "report-spot"]);
    assert_eq!(report["ok"], false);
    assert!(
        report["error"]
            .as_str()
            .expect("error")
            .contains("requires external reporting")
    );
}

#[test]
fn station_guard_allows_explicitly_armed_actions() {
    let send = run_json(&["station", "guard", "--action", "send-message", "--arm-tx"]);
    assert_eq!(send["ok"], true);
    assert_eq!(send["safety"]["tx_armed"], true);

    let report = run_json(&[
        "station",
        "guard",
        "--action",
        "report-spot",
        "--enable-reporting",
    ]);
    assert_eq!(report["ok"], true);
    assert_eq!(report["safety"]["reporting_enabled"], true);
}

#[test]
fn station_external_scaffolds_are_receive_only_by_default() {
    let report = run_json(&["station", "external", "--adapter", "fldigi"]);

    assert_eq!(report["kind"], "station-external-adapter-scaffold");
    assert_eq!(report["mode"], "fldigi-external");
    assert_eq!(report["receive_only"], true);
    assert_eq!(report["tx_enabled"], false);
    assert_eq!(report["protocol"]["kind"], "xml-rpc-http");
    assert_eq!(report["endpoint"]["host"], "127.0.0.1");
    assert_eq!(report["endpoint"]["port"], 7362);
    assert_eq!(report["safety"]["requires_explicit_arming"], true);

    let js8call = run_json(&["station", "external", "--adapter", "js8call"]);
    assert_eq!(js8call["endpoint"]["port"], 2442);
    assert_eq!(js8call["protocol"]["kind"], "tcp-json-lines");
}

#[test]
fn rig_ic705_profile_validate_and_civ_are_dry_run() {
    let dir = tempdir().expect("tempdir");
    let profile_path = dir.path().join("ic705.toml");

    let profile_write = run_json(&["rig", "ic705", "profile", "--out", &path_arg(&profile_path)]);
    assert_eq!(profile_write["kind"], "ic705-profile-write-report");
    assert_eq!(profile_write["ok"], true);
    assert!(profile_path.exists());

    let validation = run_json(&["rig", "ic705", "validate", &path_arg(&profile_path)]);
    assert_eq!(validation["kind"], "ic705-validation-report");
    assert_eq!(validation["ok"], true);
    assert_eq!(validation["profile"]["model"], "IC-705");

    let frame = run_json(&["rig", "ic705", "civ", "--operation", "read-frequency"]);
    assert_eq!(frame["kind"], "ic705-civ-frame");
    assert_eq!(frame["frame_hex"], "FE FE A4 E0 03 FD");
    assert_eq!(frame["transmit_risk"], false);

    let ptt = run_json(&["rig", "ic705", "civ", "--operation", "ptt-tx"]);
    assert_eq!(ptt["frame_hex"], "FE FE A4 E0 1C 00 01 FD");
    assert_eq!(ptt["transmit_risk"], true);

    let serial = run_json(&[
        "rig",
        "ic705",
        "civ-serial",
        "--operation",
        "read-frequency",
        "--port",
        "/dev/not-opened",
    ]);
    assert_eq!(serial["kind"], "ic705-civ-serial-report");
    assert_eq!(serial["dry_run"], true);
    assert_eq!(serial["wrote_bytes"], 0);
    assert_eq!(serial["frame"]["frame_hex"], "FE FE A4 E0 03 FD");

    let live_ptt_error = run_text_failure(&[
        "rig",
        "ic705",
        "civ-serial",
        "--operation",
        "ptt-tx",
        "--port",
        "/dev/not-opened",
        "--live",
    ]);
    assert!(live_ptt_error.contains("--allow-transmit"));
}

#[test]
fn rig_generic_hamlib_profile_validates() {
    let dir = tempdir().expect("tempdir");
    let profile_path = dir.path().join("radio.toml");

    let profile = run_json(&[
        "rig",
        "profile",
        "--model",
        "IC-7300",
        "--input-device",
        "USB Audio CODEC",
        "--output-device",
        "USB Audio CODEC",
        "--out",
        &path_arg(&profile_path),
    ]);
    assert_eq!(profile["kind"], "radio-profile-write-report");
    assert_eq!(profile["profile"]["control"]["backend"], "hamlib-rigctld");

    let validation = run_json(&["rig", "validate", &path_arg(&profile_path)]);
    assert_eq!(validation["kind"], "radio-profile-validation-report");
    assert_eq!(validation["ok"], true);
    assert_eq!(validation["profile"]["model"], "IC-7300");
}

#[test]
fn rig_hamlib_ptt_tx_requires_transmit_opt_in_before_network() {
    let error = run_text_failure(&["rig", "hamlib", "ptt-tx"]);

    assert!(error.contains("requires --allow-transmit"));
}

#[test]
fn rig_hamlib_status_reads_fake_rigctld() {
    let host = spawn_fake_rigctld(vec![
        ("f".to_owned(), vec!["14074000".to_owned()]),
        ("m".to_owned(), vec!["USB".to_owned(), "2400".to_owned()]),
        ("t".to_owned(), vec!["0".to_owned()]),
    ]);

    let report = run_json(&[
        "rig",
        "hamlib",
        "status",
        "--host",
        &host,
        "--timeout-ms",
        "5000",
    ]);

    assert_eq!(report["kind"], "hamlib-status-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["frequency_hz"], 14_074_000);
    assert_eq!(report["mode"], "USB");
    assert_eq!(report["ptt"], "rx");
}

#[test]
fn modem_live_audio_is_dry_run_by_default() {
    let report = run_json(&[
        "modem",
        "live-audio",
        "hello over usb audio",
        "--input-device",
        "USB Audio CODEC",
        "--output-device",
        "USB Audio CODEC",
    ]);

    assert_eq!(report["kind"], "live-audio-modem-report");
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["live_requested"], false);
    assert_eq!(report["played_samples"], 0);
    assert_eq!(report["captured_samples"], 0);
    assert!(report["decode"].is_null());
}

#[test]
fn chat_local_peer_script_roundtrips_over_tcp_audio_frames() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("local-peer.txt");
    let out_dir = dir.path().join("local-peer-out");
    fs::write(
        &script,
        "A CONNECT\nA SEND hello over tcp audio\nB SEND roger over tcp audio\nA DISCONNECT\n",
    )
    .expect("write script");

    let report = run_json(&[
        "chat",
        "local-peer-script",
        &path_arg(&script),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
        "--out-dir",
        &path_arg(&out_dir),
        "--gain",
        "0.9",
        "--snr-db",
        "30",
        "--drift-ppm",
        "100",
    ]);

    assert_eq!(report["kind"], "local-peer-script-report");
    assert_eq!(report["backend"], "native-local-peer");
    assert_eq!(report["ok"], true);
    assert_eq!(report["channel"]["gain"], 0.9);
    assert_eq!(report["channel"]["snr_db"], 30.0);
    assert_eq!(report["channel"]["sample_rate_drift_ppm"], 100.0);
    assert_eq!(report["station_a"]["station"]["call_sign"], "JA1TST");
    assert_eq!(report["station_b"]["station"]["call_sign"], "JA1QSO");
    assert_eq!(
        report["station_a"]["messages"]
            .as_array()
            .expect("station a messages")
            .len(),
        2
    );
    assert_eq!(
        report["station_b"]["messages"]
            .as_array()
            .expect("station b messages")
            .len(),
        2
    );
    assert_eq!(report["packets"].as_array().expect("packets").len(), 4);
    assert_eq!(
        report["packets"][1]["payload_text"],
        "MSG JA1TST JA1QSO hello over tcp audio"
    );
    assert_eq!(
        fs::read_to_string(out_dir.join("station-a/chat.log")).expect("station a log"),
        "OUT JA1QSO hello over tcp audio\nIN JA1QSO roger over tcp audio\n"
    );
    assert_eq!(
        fs::read_to_string(out_dir.join("station-b/chat.log")).expect("station b log"),
        "IN JA1TST hello over tcp audio\nOUT JA1TST roger over tcp audio\n"
    );
    assert!(
        out_dir
            .join("packets/packet-001-JA1TST-to-JA1QSO.wav")
            .exists()
    );
    assert!(
        out_dir
            .join("packets/packet-003-JA1QSO-to-JA1TST.wav")
            .exists()
    );

    let comparison = run_json(&[
        "chat",
        "compare-session-logs",
        &path_arg(&out_dir),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
    ]);
    assert_eq!(comparison["kind"], "chat-session-log-comparison-report");
    assert_eq!(comparison["ok"], true);
}

#[test]
fn chat_local_peer_app_features_survive_channel_impairment() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("local-peer-app.txt");
    let out_dir = dir.path().join("local-peer-app-out");
    fs::write(
        &script,
        concat!(
            "A BEACON monitoring 14.105 USB\n",
            "B CQ testing app packets\n",
            "A MAIL Test subject | Synthetic mailbox body\n",
            "B FILE-OFFER sample.txt 42 ",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
            "metadata only\n",
        ),
    )
    .expect("write app script");

    let report = run_json(&[
        "chat",
        "local-peer-script",
        &path_arg(&script),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
        "--out-dir",
        &path_arg(&out_dir),
        "--gain",
        "0.9",
        "--snr-db",
        "30",
        "--drift-ppm",
        "100",
    ]);

    assert_eq!(report["kind"], "local-peer-script-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["channel"]["snr_db"], 30.0);
    assert_eq!(report["channel"]["sample_rate_drift_ppm"], 100.0);
    assert_eq!(
        report["station_a_app"]["mailbox"][0]["subject"],
        "Test subject"
    );
    assert_eq!(
        report["station_a_app"]["file_offers"][0]["filename"],
        "sample.txt"
    );
    assert_eq!(report["packets"].as_array().expect("packets").len(), 4);
    let first_payload = report["packets"][0]["payload_text"]
        .as_str()
        .expect("payload text");
    assert!(first_payload.starts_with("CBAPP/1\n"));
    assert!(first_payload.contains("\"kind\":\"beacon\""));
    assert_eq!(report["packets"][0]["decode"]["ok"], true);
}

#[test]
fn chat_local_peer_file_send_writes_received_file() {
    let dir = tempdir().expect("tempdir");
    let input = dir.path().join("payload.txt");
    let script = dir.path().join("file-send.txt");
    let out_dir = dir.path().join("file-send-out");
    let payload = b"golden file payload over CBAPP chunks";
    fs::write(&input, payload).expect("write input");
    fs::write(
        &script,
        format!("A FILE-SEND {} golden transfer\n", input.display()),
    )
    .expect("write script");

    let report = run_json(&[
        "chat",
        "local-peer-script",
        &path_arg(&script),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
        "--out-dir",
        &path_arg(&out_dir),
        "--snr-db",
        "30",
        "--drift-ppm",
        "100",
    ]);

    assert_eq!(report["kind"], "local-peer-script-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["channel"]["snr_db"], 30.0);
    assert_eq!(report["channel"]["sample_rate_drift_ppm"], 100.0);
    assert_eq!(report["received_files"][0]["filename"], "payload.txt");
    assert_eq!(report["received_files"][0]["station"], "JA1QSO");
    assert!(report["packets"].as_array().expect("packets").len() > 1);
    assert_eq!(
        fs::read(out_dir.join("received/JA1QSO/payload.txt")).expect("received file"),
        payload
    );
}

#[test]
fn chat_local_node_scripts_exchange_between_processes() {
    let dir = tempdir().expect("tempdir");
    let listener_script = dir.path().join("listener.txt");
    let connector_script = dir.path().join("connector.txt");
    let ready_file = dir.path().join("listener.ready");
    let listener_out = dir.path().join("listener-out");
    let connector_out = dir.path().join("connector-out");
    fs::write(
        &listener_script,
        "EXPECT-CONNECT\nEXPECT-MSG hello from connector\nSEND roger from listener\nEXPECT-DISCONNECT\n",
    )
    .expect("write listener script");
    fs::write(
        &connector_script,
        "CONNECT\nSEND hello from connector\nEXPECT-MSG roger from listener\nDISCONNECT\n",
    )
    .expect("write connector script");

    let mut listener = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&listener_script),
            "--station",
            "ja1qso",
            "--peer",
            "ja1tst",
            "--listen",
            "127.0.0.1:0",
            "--ready-file",
            &path_arg(&ready_file),
            "--out-dir",
            &path_arg(&listener_out),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn listener");
    let address = wait_for_ready_file(&ready_file, &mut listener);

    let connector_output = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&connector_script),
            "--station",
            "ja1tst",
            "--peer",
            "ja1qso",
            "--connect",
            &address,
            "--out-dir",
            &path_arg(&connector_out),
        ])
        .output()
        .expect("run connector");
    if !connector_output.status.success() {
        let _ = listener.kill();
    }
    assert!(
        connector_output.status.success(),
        "connector failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&connector_output.stdout),
        String::from_utf8_lossy(&connector_output.stderr)
    );
    let listener_output = wait_child_output(listener, Duration::from_secs(15));
    assert!(
        listener_output.status.success(),
        "listener failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&listener_output.stdout),
        String::from_utf8_lossy(&listener_output.stderr)
    );

    let connector_report: Value =
        serde_json::from_slice(&connector_output.stdout).expect("connector json");
    let listener_report: Value =
        serde_json::from_slice(&listener_output.stdout).expect("listener json");
    assert_eq!(connector_report["kind"], "local-node-script-report");
    assert_eq!(listener_report["kind"], "local-node-script-report");
    assert_eq!(connector_report["ok"], true);
    assert_eq!(listener_report["ok"], true);
    assert_eq!(connector_report["channel"]["gain"], 1.0);
    assert_eq!(
        connector_report["station"]["station"]["call_sign"],
        "JA1TST"
    );
    assert_eq!(listener_report["station"]["station"]["call_sign"], "JA1QSO");
    assert_eq!(
        fs::read_to_string(connector_out.join("chat.log")).expect("connector log"),
        "OUT JA1QSO hello from connector\nIN JA1QSO roger from listener\n"
    );
    assert_eq!(
        fs::read_to_string(listener_out.join("chat.log")).expect("listener log"),
        "IN JA1TST hello from connector\nOUT JA1TST roger from listener\n"
    );
    assert!(
        connector_out
            .join("packets/packet-001-outbound-JA1TST-to-JA1QSO.wav")
            .exists()
    );
    assert!(
        listener_out
            .join("packets/packet-001-inbound-JA1TST-to-JA1QSO.wav")
            .exists()
    );

    let comparison = run_json(&[
        "chat",
        "compare-session-logs",
        &path_arg(&connector_out),
        &path_arg(&listener_out),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
    ]);
    assert_eq!(comparison["kind"], "chat-session-log-comparison-report");
    assert_eq!(comparison["ok"], true);
}

#[test]
fn chat_local_node_scripts_exchange_app_features_between_processes() {
    let dir = tempdir().expect("tempdir");
    let listener_script = dir.path().join("listener-app.txt");
    let connector_script = dir.path().join("connector-app.txt");
    let ready_file = dir.path().join("listener.ready");
    let listener_out = dir.path().join("listener-out");
    let connector_out = dir.path().join("connector-out");
    fs::write(
        &listener_script,
        concat!(
            "EXPECT-BEACON monitoring 14.105 USB\n",
            "CQ replying over app packets\n",
            "EXPECT-MAIL Test subject | Synthetic mailbox body\n",
            "FILE-OFFER sample.txt 42 ",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
            "metadata only\n",
        ),
    )
    .expect("write listener script");
    fs::write(
        &connector_script,
        concat!(
            "BEACON monitoring 14.105 USB\n",
            "EXPECT-CQ replying over app packets\n",
            "MAIL Test subject | Synthetic mailbox body\n",
            "EXPECT-FILE-OFFER sample.txt 42 ",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
            "metadata only\n",
        ),
    )
    .expect("write connector script");

    let mut listener = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&listener_script),
            "--station",
            "ja1qso",
            "--peer",
            "ja1tst",
            "--listen",
            "127.0.0.1:0",
            "--ready-file",
            &path_arg(&ready_file),
            "--out-dir",
            &path_arg(&listener_out),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn listener");
    let address = wait_for_ready_file(&ready_file, &mut listener);

    let connector_output = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&connector_script),
            "--station",
            "ja1tst",
            "--peer",
            "ja1qso",
            "--connect",
            &address,
            "--out-dir",
            &path_arg(&connector_out),
        ])
        .output()
        .expect("run connector");
    if !connector_output.status.success() {
        let _ = listener.kill();
    }
    assert!(
        connector_output.status.success(),
        "connector failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&connector_output.stdout),
        String::from_utf8_lossy(&connector_output.stderr)
    );
    let listener_output = wait_child_output(listener, Duration::from_secs(15));
    assert!(
        listener_output.status.success(),
        "listener failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&listener_output.stdout),
        String::from_utf8_lossy(&listener_output.stderr)
    );

    let connector_report: Value =
        serde_json::from_slice(&connector_output.stdout).expect("connector json");
    let listener_report: Value =
        serde_json::from_slice(&listener_output.stdout).expect("listener json");
    assert_eq!(connector_report["kind"], "local-node-script-report");
    assert_eq!(listener_report["kind"], "local-node-script-report");
    assert_eq!(connector_report["ok"], true);
    assert_eq!(listener_report["ok"], true);
    assert_eq!(
        connector_report["app_state"]["beacons"][0]["from"],
        "JA1TST"
    );
    assert_eq!(listener_report["app_state"]["beacons"][0]["from"], "JA1TST");
    assert_eq!(
        connector_report["app_state"]["cq_calls"][0]["from"],
        "JA1QSO"
    );
    assert_eq!(
        listener_report["app_state"]["file_offers"][0]["to"],
        "JA1TST"
    );
    assert_eq!(
        connector_report["packets"]
            .as_array()
            .expect("connector packets")
            .len(),
        4
    );
    assert_eq!(
        listener_report["packets"]
            .as_array()
            .expect("listener packets")
            .len(),
        4
    );
    let first_payload = connector_report["packets"][0]["payload_text"]
        .as_str()
        .expect("payload text");
    assert!(first_payload.starts_with("CBAPP/1\n"));
    assert!(first_payload.contains("\"kind\":\"beacon\""));
    assert!(connector_out.join("app-state.json").exists());
    assert!(listener_out.join("app-state.json").exists());
}

#[test]
fn chat_local_node_scripts_transfer_file_between_processes() {
    let dir = tempdir().expect("tempdir");
    let input = dir.path().join("payload.txt");
    let listener_script = dir.path().join("listener-file.txt");
    let connector_script = dir.path().join("connector-file.txt");
    let ready_file = dir.path().join("listener.ready");
    let listener_out = dir.path().join("listener-out");
    let connector_out = dir.path().join("connector-out");
    let payload = b"local node file transfer over chunked CBAPP packets";
    fs::write(&input, payload).expect("write input");
    let sha256 = sha256_hex(payload);
    fs::write(
        &listener_script,
        format!(
            "EXPECT-FILE-SEND payload.txt {} {} node transfer\n",
            payload.len(),
            sha256
        ),
    )
    .expect("write listener script");
    fs::write(
        &connector_script,
        format!("FILE-SEND {} node transfer\n", input.display()),
    )
    .expect("write connector script");

    let mut listener = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&listener_script),
            "--station",
            "ja1qso",
            "--peer",
            "ja1tst",
            "--listen",
            "127.0.0.1:0",
            "--ready-file",
            &path_arg(&ready_file),
            "--out-dir",
            &path_arg(&listener_out),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn listener");
    let address = wait_for_ready_file(&ready_file, &mut listener);

    let connector_output = chattybara()
        .args([
            "chat",
            "local-node-script",
            &path_arg(&connector_script),
            "--station",
            "ja1tst",
            "--peer",
            "ja1qso",
            "--connect",
            &address,
            "--out-dir",
            &path_arg(&connector_out),
        ])
        .output()
        .expect("run connector");
    if !connector_output.status.success() {
        let _ = listener.kill();
    }
    assert!(
        connector_output.status.success(),
        "connector failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&connector_output.stdout),
        String::from_utf8_lossy(&connector_output.stderr)
    );
    let listener_output = wait_child_output(listener, Duration::from_secs(15));
    assert!(
        listener_output.status.success(),
        "listener failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&listener_output.stdout),
        String::from_utf8_lossy(&listener_output.stderr)
    );

    let connector_report: Value =
        serde_json::from_slice(&connector_output.stdout).expect("connector json");
    let listener_report: Value =
        serde_json::from_slice(&listener_output.stdout).expect("listener json");
    assert_eq!(connector_report["ok"], true);
    assert_eq!(listener_report["ok"], true);
    assert_eq!(connector_report["commands"][0]["action"], "file-send");
    assert_eq!(listener_report["commands"][0]["action"], "expect-file-send");
    assert_eq!(
        listener_report["received_files"][0]["filename"],
        "payload.txt"
    );
    assert_eq!(listener_report["received_files"][0]["sha256"], sha256);
    assert!(
        connector_report["packets"]
            .as_array()
            .expect("connector packets")
            .len()
            > 1
    );
    assert_eq!(
        fs::read(listener_out.join("received/JA1QSO/payload.txt")).expect("received file"),
        payload
    );
}

#[test]
fn chat_parse_log_reports_normalized_public_log() {
    let dir = tempdir().expect("tempdir");
    let log = dir.path().join("observed-log.txt");
    fs::write(
        &log,
        "# copied from a public UI transcript by the operator\nOUT ja1qso hello\nIN ja1qso roger\n",
    )
    .expect("write log");

    let report = run_json(&["chat", "parse-log", &path_arg(&log), "--station", "ja1tst"]);

    assert_eq!(report["kind"], "chat-log-report");
    assert_eq!(report["backend"], "simple-log");
    assert_eq!(report["ok"], true);
    assert_eq!(report["transcript"]["station"]["call_sign"], "JA1TST");
    assert_eq!(report["transcript"]["peer_call"], "JA1QSO");
    assert_eq!(
        report["transcript"]["messages"]
            .as_array()
            .expect("messages")
            .len(),
        2
    );
    assert_eq!(report["transcript"]["messages"][0]["direction"], "outbound");
    assert_eq!(report["transcript"]["messages"][1]["direction"], "inbound");
}

#[test]
fn chat_parse_log_fails_on_unknown_normalized_line() {
    let dir = tempdir().expect("tempdir");
    let log = dir.path().join("observed-log.txt");
    fs::write(&log, "CHAT ja1qso hello\n").expect("write log");

    let report = run_json_failure(&["chat", "parse-log", &path_arg(&log), "--station", "ja1tst"]);

    assert_eq!(report["kind"], "chat-log-report");
    assert_eq!(report["ok"], false);
    assert_eq!(report["commands"][0]["ok"], false);
    assert!(
        report["commands"][0]["error"]
            .as_str()
            .expect("error")
            .contains("unknown chat script command")
    );
}

#[test]
fn chat_compare_script_log_matches_normalized_public_log() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("chat-script.txt");
    let log = dir.path().join("observed-log.txt");
    fs::write(
        &script,
        "CONNECT ja1qso\nSEND hello from chattybara\nRX ja1qso roger from peer\nDISCONNECT\n",
    )
    .expect("write script");
    fs::write(
        &log,
        "OUT ja1qso hello from chattybara\nIN ja1qso roger from peer\n",
    )
    .expect("write log");

    let report = run_json(&[
        "chat",
        "compare-script-log",
        &path_arg(&script),
        &path_arg(&log),
        "--station",
        "ja1tst",
    ]);

    assert_eq!(report["kind"], "chat-script-log-comparison-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["expected"]["backend"], "fake");
    assert_eq!(report["observed"]["backend"], "simple-log");
    assert_eq!(report["comparison"]["expected_message_count"], 2);
    assert_eq!(report["comparison"]["observed_message_count"], 2);
    assert_eq!(
        report["comparison"]["mismatches"]
            .as_array()
            .expect("mismatches")
            .len(),
        0
    );
}

#[test]
fn chat_compare_script_log_fails_on_message_mismatch() {
    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("chat-script.txt");
    let log = dir.path().join("observed-log.txt");
    fs::write(&script, "CONNECT ja1qso\nSEND hello\nRX ja1qso roger\n").expect("write script");
    fs::write(&log, "OUT ja1qso hello\nIN ja1qso nope\n").expect("write log");

    let report = run_json_failure(&[
        "chat",
        "compare-script-log",
        &path_arg(&script),
        &path_arg(&log),
        "--station",
        "ja1tst",
    ]);

    assert_eq!(report["kind"], "chat-script-log-comparison-report");
    assert_eq!(report["ok"], false);
    assert_eq!(report["comparison"]["mismatches"][0]["message_index"], 2);
    assert_eq!(report["comparison"]["mismatches"][0]["field"], "text");
}

#[test]
fn chat_compare_peer_logs_matches_two_station_views() {
    let dir = tempdir().expect("tempdir");
    let station_a_log = dir.path().join("station-a-log.txt");
    let station_b_log = dir.path().join("station-b-log.txt");
    fs::write(
        &station_a_log,
        "OUT ja1qso hello from a\nIN ja1qso roger from b\n",
    )
    .expect("write station a log");
    fs::write(
        &station_b_log,
        "IN ja1tst hello from a\nOUT ja1tst roger from b\n",
    )
    .expect("write station b log");

    let report = run_json(&[
        "chat",
        "compare-peer-logs",
        &path_arg(&station_a_log),
        &path_arg(&station_b_log),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
    ]);

    assert_eq!(report["kind"], "chat-peer-log-comparison-report");
    assert_eq!(report["ok"], true);
    assert_eq!(
        report["mismatches"].as_array().expect("mismatches").len(),
        0
    );
    assert_eq!(
        report["station_a"]["transcript"]["station"]["call_sign"],
        "JA1TST"
    );
    assert_eq!(
        report["station_b"]["transcript"]["station"]["call_sign"],
        "JA1QSO"
    );
}

#[test]
fn chat_compare_peer_logs_fails_on_cross_station_mismatch() {
    let dir = tempdir().expect("tempdir");
    let station_a_log = dir.path().join("station-a-log.txt");
    let station_b_log = dir.path().join("station-b-log.txt");
    fs::write(&station_a_log, "OUT ja1qso hello\n").expect("write station a log");
    fs::write(&station_b_log, "IN ja1tst nope\n").expect("write station b log");

    let report = run_json_failure(&[
        "chat",
        "compare-peer-logs",
        &path_arg(&station_a_log),
        &path_arg(&station_b_log),
        "--station-a",
        "ja1tst",
        "--station-b",
        "ja1qso",
    ]);

    assert_eq!(report["kind"], "chat-peer-log-comparison-report");
    assert_eq!(report["ok"], false);
    assert_eq!(report["mismatches"][0]["message_index"], 1);
    assert_eq!(report["mismatches"][0]["field"], "text");
    assert_eq!(report["mismatches"][0]["station_a"], "hello");
    assert_eq!(report["mismatches"][0]["station_b"], "nope");
}

#[test]
fn corpus_observation_validation_reports_files() {
    let dir = tempdir().expect("tempdir");
    fs::write(dir.path().join("host-transcript.txt"), "PING\nPONG\n").expect("transcript");
    let manifest = dir.path().join("observation.toml");
    fs::write(
        &manifest,
        r#"
schema_version = 1
observation_id = "synthetic-observation-001"
generated_payload = "hello"
expected_behavior = "host reply"
provenance = "clean-inferred"

[endpoint]
modem = "external-modem"
modem_version = "unknown"
os = "macos"
audio_path = "virtual-loopback"
host_path = "tcp-localhost"

[[files]]
path = "host-transcript.txt"
role = "host-transcript"
media_type = "text/plain"
"#,
    )
    .expect("manifest");

    let report = run_json(&["corpus", "observation", "validate", &path_arg(&manifest)]);

    assert_eq!(report["kind"], "observation-validation-report");
    assert_eq!(report["observation_id"], "synthetic-observation-001");
    assert_eq!(report["files"][0]["role"], "host-transcript");
}

#[test]
fn dsp_commands_report_stable_synthetic_tone() {
    let dir = tempdir().expect("tempdir");
    let tone = dir.path().join("tone.wav");
    let tone_arg = path_arg(&tone);
    run_json(&[
        "fixture",
        "synth",
        &tone_arg,
        "--kind",
        "tone-burst",
        "--frequency",
        "1000",
    ]);

    let tone_report = run_json(&["dsp", "tone", &tone_arg]);
    assert_eq!(tone_report["frequency_hz"], 1000.0);

    let track = run_json(&["dsp", "track", &tone_arg]);
    assert_eq!(track["kind"], "dominant-frequency-track");
    assert!(!track["frames"].as_array().expect("frames").is_empty());

    let soft = run_json(&[
        "dsp",
        "soft",
        "--mark",
        "1000",
        "--space",
        "1500",
        "--samples-per-symbol",
        "80",
        &tone_arg,
    ]);
    assert_eq!(soft["kind"], "soft-decision-trace");
    assert!(!soft["symbols"].as_array().expect("symbols").is_empty());

    let filtered = dir.path().join("tone-filtered.wav");
    let bandpass = run_json(&[
        "dsp",
        "bandpass",
        "--low",
        "900",
        "--high",
        "1100",
        &tone_arg,
        &path_arg(&filtered),
    ]);
    assert_eq!(bandpass["kind"], "bandpass-report");
    assert_eq!(bandpass["peak_frequency_hz"], 1000.0);
    assert!(filtered.exists());
}

#[test]
fn modem_packet_encode_decode_roundtrips_without_hardware() {
    let dir = tempdir().expect("tempdir");
    let packet = dir.path().join("packet.wav");
    let delayed = dir.path().join("packet-delayed.wav");
    let impaired = dir.path().join("packet-impaired.wav");

    let encode = run_json(&["modem", "encode", "hello chattybara", &path_arg(&packet)]);

    assert_eq!(encode["kind"], "packet-encode-report");
    assert_eq!(encode["payload_len"], 16);
    assert_eq!(encode["payload_text"], "hello chattybara");
    assert!(packet.exists());

    let decode = run_json(&["modem", "decode", &path_arg(&packet)]);

    assert_eq!(decode["kind"], "packet-decode-report");
    assert_eq!(decode["ok"], true);
    assert_eq!(decode["payload_text"], "hello chattybara");
    assert_eq!(decode["crc_expected"], decode["crc_actual"]);

    run_json(&[
        "audio",
        "loopback",
        "--latency-frames",
        "60",
        "--gain",
        "0.9",
        &path_arg(&packet),
        &path_arg(&delayed),
    ]);
    let delayed_decode = run_json(&["modem", "decode", &path_arg(&delayed)]);

    assert_eq!(delayed_decode["ok"], true);
    assert_eq!(delayed_decode["payload_text"], "hello chattybara");
    assert!(delayed_decode["sample_offset"].as_u64().expect("offset") > 0);

    run_json(&[
        "simulate",
        "channel",
        "--gain",
        "0.8",
        "--snr",
        "30",
        "--sample-rate-drift-ppm",
        "100",
        &path_arg(&packet),
        &path_arg(&impaired),
    ]);
    let impaired_decode = run_json(&["modem", "decode", &path_arg(&impaired)]);

    assert_eq!(impaired_decode["ok"], true);
    assert_eq!(impaired_decode["payload_text"], "hello chattybara");

    let roundtrip = run_json(&["modem", "roundtrip", "hello chattybara"]);

    assert_eq!(roundtrip["kind"], "packet-roundtrip-report");
    assert_eq!(roundtrip["ok"], true);
    assert_eq!(roundtrip["decode"]["payload_text"], "hello chattybara");

    let sweep_dir = dir.path().join("sweep");
    let sweep = run_json(&["modem", "sweep", "hello chattybara", &path_arg(&sweep_dir)]);

    assert_eq!(sweep["kind"], "packet-sweep-report");
    assert_eq!(sweep["ok"], true);
    assert_eq!(sweep["case_count"], 9);
    assert_eq!(sweep["passed_count"], 9);
    assert!(sweep_dir.join("sweep-report.json").exists());
    assert!(sweep_dir.join("baseline.wav").exists());
}

#[test]
fn modem_samples_create_rx_tx_audio_fixture_set() {
    let dir = tempdir().expect("tempdir");
    let samples_dir = dir.path().join("samples");

    let report = run_json(&[
        "modem",
        "samples",
        "hello chattybara",
        &path_arg(&samples_dir),
    ]);

    assert_eq!(report["kind"], "packet-audio-samples-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["payload_text"], "hello chattybara");
    assert_eq!(report["decode"]["tx_packet"]["ok"], true);
    assert_eq!(
        report["decode"]["tx_packet"]["payload_text"],
        "hello chattybara"
    );
    assert_eq!(report["decode"]["rx_clean"]["ok"], true);
    assert_eq!(report["decode"]["rx_loopback"]["ok"], true);
    assert_eq!(report["decode"]["rx_impaired"]["ok"], true);
    assert_eq!(report["decode"]["rx_silence"]["ok"], false);
    assert!(samples_dir.join("tx-packet.wav").exists());
    assert!(samples_dir.join("rx-clean.wav").exists());
    assert!(samples_dir.join("rx-loopback.wav").exists());
    assert!(samples_dir.join("rx-impaired.wav").exists());
    assert!(samples_dir.join("rx-silence.wav").exists());
    assert!(samples_dir.join("samples-report.json").exists());
    assert!(samples_dir.join("rx-impaired.decode.json").exists());

    let impaired_decode = run_json(&[
        "modem",
        "decode",
        &path_arg(&samples_dir.join("rx-impaired.wav")),
    ]);
    assert_eq!(impaired_decode["ok"], true);
    assert_eq!(impaired_decode["payload_text"], "hello chattybara");
}

#[test]
fn app_link_simulator_exercises_retries_fragments_and_files() {
    let report = run_json(&[
        "simulate",
        "app-link",
        "--payload-bytes",
        "180",
        "--drop-first-attempt",
        "--duplicate-deliveries",
    ]);

    assert_eq!(report["kind"], "app-link-simulation-report");
    assert_eq!(report["station"], "JA1TST");
    assert_eq!(report["peer"], "JA1QSO");
    assert_eq!(report["payload_bytes"], 180);
    assert!(report["packet_count"].as_u64().expect("packet count") > 4);
    assert!(report["max_encoded_bytes"].as_u64().expect("encoded bytes") <= 512);
    assert_eq!(report["fragment"]["byte_count"], 180);
    assert_eq!(report["file"]["filename"], "synthetic.bin");
    assert_eq!(report["file"]["byte_count"], 180);
    assert_eq!(
        report["reliability"]["failed_ids"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(report["reliability"]["delivered"], report["packet_count"]);
    assert_eq!(
        report["reliability"]["acknowledged"],
        report["packet_count"]
    );
    assert_eq!(
        report["reliability"]["duplicate_packets"],
        report["packet_count"]
    );
    assert!(
        report["reliability"]["timeouts"]
            .as_u64()
            .expect("timeouts")
            > 0
    );

    let failed = run_json(&[
        "simulate",
        "app-link",
        "--payload-bytes",
        "64",
        "--max-retries",
        "1",
        "--timeout-ticks",
        "2",
        "--drop-all-attempts",
    ]);
    let failed_packet_count = failed["packet_count"].as_u64().expect("packet count");
    assert_eq!(failed["reliability"]["delivered"], 0);
    assert_eq!(failed["reliability"]["acknowledged"], 0);
    assert_eq!(failed["reliability"]["attempts"], failed_packet_count * 2);
    assert_eq!(failed["reliability"]["timeouts"], failed_packet_count * 4);
    assert_eq!(
        failed["reliability"]["failed_ids"]
            .as_array()
            .expect("failed ids")
            .len() as u64,
        failed_packet_count
    );
}

#[test]
fn modem_decode_fails_cleanly_without_sync() {
    let dir = tempdir().expect("tempdir");
    let silence = dir.path().join("silence.wav");
    run_json(&["fixture", "synth", &path_arg(&silence), "--kind", "silence"]);

    let decode = run_json_failure(&["modem", "decode", &path_arg(&silence)]);

    assert_eq!(decode["kind"], "packet-decode-report");
    assert_eq!(decode["ok"], false);
    assert_eq!(
        decode["errors"][0],
        "packet preamble/sync pattern not found"
    );
}

#[test]
fn fixture_suite_generates_valid_regression_corpus() {
    let dir = tempdir().expect("tempdir");
    let suite_dir = dir.path().join("suite");

    let suite = run_json(&["fixture", "suite", &path_arg(&suite_dir)]);

    assert_eq!(suite["kind"], "fixture-suite-report");
    assert_eq!(suite["fixtures"].as_array().expect("fixtures").len(), 5);
    assert!(suite_dir.join("manifest.toml").exists());
    assert!(suite_dir.join("host-script.txt").exists());

    let validation = run_text(&["corpus", "validate", &path_arg(&suite_dir)]);
    assert!(validation.contains("validated 5 fixture(s)"));

    let verification = run_json(&["corpus", "verify", &path_arg(&suite_dir)]);
    assert_eq!(verification["kind"], "corpus-verification-report");
    assert_eq!(verification["ok"], true);
    assert_eq!(
        verification["fixtures"].as_array().expect("fixtures").len(),
        5
    );
}

#[test]
fn lab_run_writes_complete_no_hardware_report() {
    let dir = tempdir().expect("tempdir");
    let lab_dir = dir.path().join("lab");

    let report = run_json(&["lab", "run", &path_arg(&lab_dir)]);

    assert_eq!(report["kind"], "lab-run-report");
    assert_eq!(report["ok"], true);
    assert_eq!(report["suite"]["kind"], "fixture-suite-report");
    assert_eq!(report["verification"]["ok"], true);
    assert_eq!(report["host"]["ok"], true);
    assert_eq!(report["chat"]["ok"], true);
    assert_eq!(report["modem"]["ok"], true);
    assert_eq!(
        report["chat"]["fake_script"]["transcript"]["messages"]
            .as_array()
            .expect("messages")
            .len(),
        2
    );
    assert_eq!(report["chat"]["app_script"]["ok"], true);
    assert_eq!(
        report["chat"]["app_script"]["state"]["beacons"]
            .as_array()
            .expect("app beacons")
            .len(),
        1
    );
    assert_eq!(
        report["chat"]["app_script"]["state"]["file_offers"][0]["filename"],
        "sample.txt"
    );
    assert_eq!(report["chat"]["peer_log_comparison"]["ok"], true);
    assert_eq!(report["chat"]["local_peer"]["ok"], true);
    assert_eq!(report["chat"]["local_peer_app"]["ok"], true);
    assert_eq!(
        report["chat"]["local_peer"]["packets"]
            .as_array()
            .expect("local peer packets")
            .len(),
        4
    );
    assert_eq!(
        report["chat"]["local_peer_app"]["packets"]
            .as_array()
            .expect("local peer app packets")
            .len(),
        4
    );
    assert_eq!(
        report["chat"]["local_peer_app"]["station_a_app"]["file_offers"][0]["filename"],
        "sample.txt"
    );
    assert_eq!(
        report["modem"]["direct_decode"]["payload_text"],
        "hello chattybara"
    );
    assert_eq!(
        report["modem"]["impaired_decode"]["payload_text"],
        "hello chattybara"
    );
    assert_eq!(report["artifacts"].as_array().expect("artifacts").len(), 5);
    assert!(lab_dir.join("lab-report.json").exists());
    assert!(
        lab_dir
            .join("artifacts/traces/tone-burst.trace.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/classifications/tone-burst.classification.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/pipelines/tone-burst.pipeline.json")
            .exists()
    );
    assert!(lab_dir.join("artifacts/dsp/tone-burst.tone.json").exists());
    assert!(lab_dir.join("artifacts/modem/packet.wav").exists());
    assert!(lab_dir.join("artifacts/modem/packet-impaired.wav").exists());
    assert!(lab_dir.join("artifacts/chat/chat-lab-report.json").exists());
    assert!(
        lab_dir
            .join("artifacts/chat/app-script-report.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/chat/local-peer/session.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/chat/local-peer-app/session.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/chat/local-peer/packets/packet-002-JA1TST-to-JA1QSO.wav")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/chat/local-peer-app/station-a/app-state.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/chat/peer-log-comparison.json")
            .exists()
    );
    assert!(
        lab_dir
            .join("artifacts/modem/modem-lab-report.json")
            .exists()
    );
}

#[test]
fn lab_snapshot_and_compare_pass_for_same_run() {
    let dir = tempdir().expect("tempdir");
    let lab_dir = dir.path().join("lab");
    let report_path = lab_dir.join("lab-report.json");
    let snapshot_path = lab_dir.join("lab-snapshot.json");

    run_json(&["lab", "run", &path_arg(&lab_dir)]);

    let snapshot = run_json(&[
        "lab",
        "snapshot",
        &path_arg(&report_path),
        "--out",
        &path_arg(&snapshot_path),
    ]);

    assert_eq!(snapshot["kind"], "lab-snapshot");
    assert_eq!(snapshot["fixture_count"], 5);
    assert_eq!(snapshot["host_ok"], true);
    assert_eq!(snapshot["chat_ok"], true);
    assert_eq!(snapshot["chat"]["message_count"], 2);
    assert_eq!(snapshot["chat"]["app_script_ok"], true);
    assert_eq!(snapshot["chat"]["app_beacon_count"], 1);
    assert_eq!(snapshot["chat"]["app_cq_count"], 1);
    assert_eq!(snapshot["chat"]["app_mailbox_count"], 1);
    assert_eq!(snapshot["chat"]["app_file_offer_count"], 1);
    assert_eq!(snapshot["chat"]["local_peer_ok"], true);
    assert_eq!(snapshot["chat"]["local_peer_app_ok"], true);
    assert_eq!(snapshot["chat"]["local_peer_message_count"], 2);
    assert_eq!(snapshot["chat"]["local_peer_packet_count"], 4);
    assert_eq!(snapshot["chat"]["local_peer_app_packet_count"], 4);
    assert_eq!(snapshot["chat"]["local_peer_app_beacon_count"], 1);
    assert_eq!(snapshot["chat"]["local_peer_app_file_offer_count"], 1);
    assert_eq!(
        snapshot["chat"]["local_peer_packets"][1]["payload_text"],
        "MSG JA1TST JA1QSO hello from chattybara"
    );
    assert_eq!(snapshot["chat"]["peer_mismatch_count"], 0);
    assert_eq!(snapshot["modem"]["ok"], true);
    assert_eq!(
        snapshot["modem"]["impaired_decode"]["payload_hex"],
        "68656c6c6f2063686174747962617261"
    );
    assert!(snapshot_path.exists());

    let compare = run_json(&[
        "lab",
        "compare",
        &path_arg(&snapshot_path),
        &path_arg(&report_path),
    ]);

    assert_eq!(compare["kind"], "lab-compare-report");
    assert_eq!(compare["ok"], true);
    assert_eq!(compare["difference_count"], 0);
}

#[test]
fn lab_compare_fails_on_candidate_drift() {
    let dir = tempdir().expect("tempdir");
    let lab_dir = dir.path().join("lab");
    let report_path = lab_dir.join("lab-report.json");
    let snapshot_path = lab_dir.join("lab-snapshot.json");
    let drifted_path = lab_dir.join("lab-snapshot-drifted.json");

    run_json(&["lab", "run", &path_arg(&lab_dir)]);
    let snapshot = run_json(&[
        "lab",
        "snapshot",
        &path_arg(&report_path),
        "--out",
        &path_arg(&snapshot_path),
    ]);
    let mut drifted = snapshot.clone();
    drifted["fixtures"][0]["candidate_count"] = Value::from(99);
    fs::write(
        &drifted_path,
        serde_json::to_string_pretty(&drifted).expect("snapshot json"),
    )
    .expect("write drifted snapshot");

    let compare = run_json_failure(&[
        "lab",
        "compare",
        &path_arg(&snapshot_path),
        &path_arg(&drifted_path),
    ]);

    assert_eq!(compare["kind"], "lab-compare-report");
    assert_eq!(compare["ok"], false);
    assert!(
        compare["differences"]
            .as_array()
            .expect("differences")
            .iter()
            .any(|difference| difference["path"]
                .as_str()
                .expect("path")
                .ends_with(".candidate_count"))
    );
}

#[test]
fn corpus_verify_fails_on_expectation_mismatch() {
    let dir = tempdir().expect("tempdir");
    let wav = dir.path().join("tone.wav");
    let manifest = dir.path().join("manifest.toml");
    run_json(&[
        "fixture",
        "synth",
        &path_arg(&wav),
        "--kind",
        "tone-burst",
        "--frequency",
        "1000",
    ]);
    fs::write(
        &manifest,
        r#"
schema_version = 1

[[fixtures]]
id = "tone"
audio = "tone.wav"
sample_rate = 8000
channels = 1
payload = "synthetic tone"
provenance = "clean-public"
expected = "no-signal"
"#,
    )
    .expect("manifest");

    let verification = run_json_failure(&["corpus", "verify", &path_arg(&manifest)]);

    assert_eq!(verification["kind"], "corpus-verification-report");
    assert_eq!(verification["ok"], false);
    assert_eq!(verification["fixtures"][0]["passed"], false);
}

#[test]
fn corpus_audit_passes_checked_in_corpus() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");

    let audit = run_json(&["corpus", "audit", &path_arg(repo_root)]);

    assert_eq!(audit["kind"], "corpus-audit-report");
    assert_eq!(audit["ok"], true);
    assert!(
        audit["checked_manifests"]
            .as_array()
            .expect("checked")
            .len()
            >= 2
    );
}

#[test]
fn corpus_fixture_validation_text_stays_readable() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");
    let manifest = repo_root.join("corpus/fixtures-small/manifest.example.toml");

    let stdout = run_text(&["corpus", "validate", &path_arg(&manifest)]);

    assert!(stdout.contains("validated 1 fixture(s)"));
    assert!(stdout.contains("synthetic-silence-001"));
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn wait_for_ready_file(path: &Path, child: &mut std::process::Child) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
        if let Some(status) = child.try_wait().expect("poll listener") {
            panic!("listener exited before writing ready file: {status}");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timed out waiting for {}", path.display());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_child_output(mut child: std::process::Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("child output");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("killed child output");
            panic!(
                "child timed out\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_fake_rigctld(commands: Vec<(String, Vec<String>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rigctld");
    let address = listener
        .local_addr()
        .expect("fake rigctld addr")
        .to_string();
    thread::spawn(move || {
        let expected_count = commands.len();
        let mut replies_by_command = commands.into_iter().fold(
            HashMap::<String, VecDeque<Vec<String>>>::new(),
            |mut by_command, (command, replies)| {
                by_command.entry(command).or_default().push_back(replies);
                by_command
            },
        );
        let mut handled = 0_usize;
        while handled < expected_count {
            let (stream, _) = listener.accept().expect("accept fake rigctld client");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            reader.read_line(&mut line).expect("read rigctld command");
            let command = line.trim_end().to_owned();
            let mut stream = reader.into_inner();
            let Some(replies) = replies_by_command
                .get_mut(&command)
                .and_then(VecDeque::pop_front)
            else {
                writeln!(stream, "RPRT -1").expect("write rigctld error");
                continue;
            };
            for reply in replies {
                writeln!(stream, "{reply}").expect("write rigctld reply");
            }
            stream.flush().expect("flush rigctld reply");
            handled += 1;
        }
    });
    address
}

fn read_cr_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut bytes = Vec::new();
    reader.read_until(b'\r', &mut bytes).expect("read line");
    while matches!(bytes.last(), Some(b'\r' | b'\n')) {
        bytes.pop();
    }
    String::from_utf8(bytes).expect("utf8")
}

fn read_nonempty_cr_line(reader: &mut BufReader<TcpStream>) -> String {
    for _ in 0..4 {
        let line = read_cr_line(reader);
        if !line.is_empty() {
            return line;
        }
    }
    panic!("expected non-empty CR-terminated line")
}

fn b2f_checksum(lines: &[String]) -> u8 {
    let mut sum = 0_i64;
    for line in lines {
        for byte in line.bytes() {
            sum += i64::from(byte);
        }
        sum += i64::from(b'\r');
    }
    ((-sum) & 0xff) as u8
}

fn b2_fixture_message(mid: &str, from: &str, to: &str, subject: &str, body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    write!(
        &mut payload,
        "Mid: {mid}\r\nDate: 2026/05/13 10:00\r\nType: Private\r\nFrom: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nMbo: {from}\r\nBody: {}\r\n\r\n",
        body.len()
    )
    .expect("write b2 fixture");
    payload.extend_from_slice(body);
    payload
}

fn b2_lzhuf_fixture_payload(payload: &[u8]) -> Vec<u8> {
    let compressed = retrocompressor::lzss_huff::compress_slice(
        payload,
        &retrocompressor::lzss_huff::STD_OPTIONS,
    )
    .expect("compress b2 fixture");
    let crc = b2_crc16(&compressed);
    let mut out = Vec::with_capacity(compressed.len() + 2);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

fn b2_transfer_fixture(title: &str, compressed: &[u8]) -> Vec<u8> {
    let mut transfer = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(title.as_bytes());
    header.push(0);
    header.extend_from_slice(b"0");
    header.push(0);
    transfer.push(0x01);
    transfer.push(header.len() as u8);
    transfer.extend_from_slice(&header);
    let mut checksum = 0_u16;
    for chunk in compressed.chunks(250) {
        transfer.push(0x02);
        transfer.push(chunk.len() as u8);
        for byte in chunk {
            checksum = (checksum + u16::from(*byte)) & 0xff;
        }
        transfer.extend_from_slice(chunk);
    }
    transfer.push(0x04);
    transfer.push((0_u16.wrapping_sub(checksum) & 0xff) as u8);
    transfer
}

fn b2_crc16(payload: &[u8]) -> u16 {
    let mut sum = 0_u16;
    for byte in payload.iter().copied().chain([0, 0]) {
        let table = crc16_ccitt_table_value((sum >> 8) as u8);
        sum = ((sum << 8) & 0xff00) ^ table ^ u16::from(byte);
    }
    sum
}

fn crc16_ccitt_table_value(index: u8) -> u16 {
    let mut value = u16::from(index) << 8;
    for _ in 0..8 {
        if (value & 0x8000) != 0 {
            value = (value << 1) ^ 0x1021;
        } else {
            value <<= 1;
        }
    }
    value
}
