//! `ferriscribe-backup` — standalone CLI for FerriScribe's off-machine
//! backup (independent of the Tauri app so schedules survive the app
//! being closed; R2).
//!
//! Top-level flow docs live in the README section "Off-machine backup".

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use medical_backup::agent::{self, AgentConfig};
use medical_backup::client::BackupClient;
use medical_backup::escrow;
use medical_backup::keys;
use medical_backup::snapshot::{self, BuildOptions};
use medical_backup::{drill, schedule};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((cmd, rest)) => (cmd.as_str(), rest),
        None => {
            print_usage();
            return ExitCode::from(2);
        }
    };
    let flags = Flags::parse(rest);

    let result = match cmd {
        "escrow" => cmd_escrow(&flags),
        "backup" => run_blocking(cmd_backup(&flags)),
        "backup-and-push" => run_blocking(cmd_backup_and_push(&flags)),
        "push" => run_blocking(cmd_push(&flags)),
        "pull" => run_blocking(cmd_pull(&flags)),
        "verify" => run_blocking(cmd_verify(&flags)),
        "restore" => run_blocking(cmd_restore(&flags)),
        "drill" => run_blocking(cmd_drill(&flags)),
        "serve" => run_blocking(cmd_serve(&flags)),
        "install-schedule" => cmd_install_schedule(&flags),
        "uninstall-schedule" => cmd_uninstall_schedule(),
        "--help" | "-h" | "help" => {
            print_usage();
            Ok(())
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!(
        "ferriscribe-backup — encrypted off-machine backup for FerriScribe

  escrow init [--out-dir DIR]              generate the wrapping key (keychain) + write
                                            the recovery sheet and USB escrow files
  escrow verify --file PATH                verify an escrow artifact (sheet or USB)
  backup [--data-dir DIR] [--recordings-dir DIR] [--out DIR]
                                            build an encrypted snapshot locally
  backup-and-push ... --url URL --token T  build + push to the agent + run a local drill
  push --url URL --token T --snapshot-dir DIR
  pull --url URL --token T [--id ID] --out DIR (--escrow-file F | --key-hex H)
  verify --snapshot-dir DIR (--escrow-file F | --key-hex H)
  restore --snapshot-dir DIR --dest DIR (--escrow-file F | --key-hex H)
  drill --snapshot-dir DIR ... | --url URL --token T ...
                                            restore to a temp dir and verify; exits 1
                                            loudly on any failure
  serve --root DIR --bind IP:PORT           run the append-only target agent (tokens via
                                            FERRISCRIBE_BACKUP_APPEND_TOKEN /
                                            FERRISCRIBE_BACKUP_ADMIN_TOKEN)
  install-schedule --hour H --minute M [--url URL --token T]
                                            install the launchd daily backup agent
  uninstall-schedule                       remove the launchd agent"
    );
}

// ── flag parsing (deliberately tiny; no clap dependency) ─────────────────

struct Flags {
    values: std::collections::HashMap<String, String>,
    positional: Vec<String>,
}

impl Flags {
    fn parse(args: &[String]) -> Self {
        let mut values = std::collections::HashMap::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(name) = a.strip_prefix("--") {
                let next = args.get(i + 1);
                if let Some(v) = next.filter(|v| !v.starts_with("--")) {
                    values.insert(name.to_string(), v.clone());
                    i += 2;
                } else {
                    values.insert(name.to_string(), String::new());
                    i += 1;
                }
            } else {
                positional.push(a.clone());
                i += 1;
            }
        }
        Self { values, positional }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }

    fn req(&self, name: &str) -> Result<String, medical_backup::BackupError> {
        self.get(name).map(|s| s.to_string()).ok_or_else(|| {
            medical_backup::BackupError::Escrow(format!("missing required flag --{name}"))
        })
    }
}

type CmdResult = Result<(), medical_backup::BackupError>;

fn run_blocking<F: std::future::Future<Output = CmdResult>>(fut: F) -> CmdResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

// ── key resolution ───────────────────────────────────────────────────────

/// Resolve the wrapping key from --escrow-file / --key-hex / keychain.
fn resolve_wrapping_key(flags: &Flags) -> medical_backup::BackupResult<[u8; 32]> {
    if let Some(file) = flags.get("escrow-file") {
        return escrow::read_key_from_artifact(Path::new(file));
    }
    if let Some(hexkey) = flags.get("key-hex") {
        let bytes = hex::decode(hexkey)
            .map_err(|e| medical_backup::BackupError::Escrow(format!("--key-hex: {e}")))?;
        if bytes.len() != 32 {
            return Err(medical_backup::BackupError::Escrow(
                "--key-hex must be 64 hex characters".into(),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    keys::load_wrapping_key()
}

// ── subcommands ──────────────────────────────────────────────────────────

fn cmd_escrow(flags: &Flags) -> CmdResult {
    match flags.positional.first().map(|s| s.as_str()) {
        Some("init") => {
            let out_dir = flags
                .get("out-dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
            let wrapping = keys::load_or_create_wrapping_key()?;
            let sheet = out_dir.join(escrow::SHEET_FILENAME);
            let usb = out_dir.join(escrow::USB_FILENAME);
            escrow::write_recovery_sheet(&sheet, &wrapping)?;
            escrow::write_usb_file(&usb, &wrapping)?;
            println!("recovery sheet : {}", sheet.display());
            println!("  → PRINT it and store it in a safe, off-machine, fire-protected place.");
            println!("usb escrow file: {}", usb.display());
            println!("  → COPY it to an offline USB stick (not left plugged in).");
            println!("Both artifacts are independently sufficient and independently verifiable:");
            println!("  ferriscribe-backup escrow verify --file <path>");
            Ok(())
        }
        Some("verify") => {
            let file = flags.req("file")?;
            let expected = keys::load_wrapping_key().ok();
            let status = escrow::verify_artifact(Path::new(&file), expected.as_ref())?;
            println!("{status}");
            Ok(())
        }
        _ => Err(medical_backup::BackupError::Escrow(
            "escrow requires `init` or `verify --file PATH`".into(),
        )),
    }
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|d| d.join("rust-medical-assistant"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn build_options_from(flags: &Flags) -> medical_backup::BackupResult<BuildOptions> {
    let data_dir = flags
        .get("data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    let recordings_dir = flags
        .get("recordings-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("recordings"));
    let out = flags
        .get("out")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("backups"));
    std::fs::create_dir_all(&out)?;
    Ok(BuildOptions {
        db_path: data_dir.join("medical.db"),
        recordings_dir,
        keystore_path: Some(data_dir.join("config").join("keys.json")),
        dest_dir: out,
        db_key: medical_security::keychain::get_or_create_db_key()?,
        wrapping_key: keys::load_or_create_wrapping_key()?,
    })
}

async fn cmd_backup(flags: &Flags) -> CmdResult {
    let opts = build_options_from(flags)?;
    let receipt = snapshot::build_snapshot(&opts)?;
    println!(
        "snapshot {} built at {}",
        receipt.snapshot_id,
        opts.dest_dir.join(&receipt.snapshot_id).display()
    );
    println!(
        "  files: {}, recordings rows: {}, bytes: {}",
        receipt.file_count, receipt.recording_count, receipt.total_bytes
    );
    Ok(())
}

/// The scheduled unit: build → push (if configured) → drill the local
/// snapshot. A drill failure is a loud non-zero exit so launchd logs and
/// the user notices (R4).
async fn cmd_backup_and_push(flags: &Flags) -> CmdResult {
    let opts = build_options_from(flags)?;
    let receipt = snapshot::build_snapshot(&opts)?;
    let local_dir = opts.dest_dir.join(&receipt.snapshot_id);
    println!(
        "snapshot {} built ({} bytes)",
        receipt.snapshot_id, receipt.total_bytes
    );

    if let Some(url) = flags.get("url") {
        let token = flags.req("token")?;
        let client = BackupClient::new(url, token);
        client.push_snapshot(&local_dir).await?;
        println!("pushed to {url}");
    }

    let outcome = drill::run_drill(&local_dir, &opts.wrapping_key);
    for check in &outcome.checks {
        println!("  ✓ {check}");
    }
    if outcome.passed {
        println!("drill passed for {}", outcome.snapshot_id);
        Ok(())
    } else {
        for failure in &outcome.failures {
            eprintln!("  ✗ {failure}");
        }
        Err(medical_backup::BackupError::Verification(
            "restore drill failed".into(),
        ))
    }
}

async fn cmd_push(flags: &Flags) -> CmdResult {
    let url = flags.req("url")?;
    let token = flags.req("token")?;
    let dir = flags.req("snapshot-dir")?;
    let receipt = BackupClient::new(url, token)
        .push_snapshot(Path::new(&dir))
        .await?;
    println!("pushed {}", receipt.snapshot_id);
    Ok(())
}

async fn cmd_pull(flags: &Flags) -> CmdResult {
    let url = flags.req("url")?;
    let token = flags.req("token")?;
    let out = flags.req("out")?;
    let wrapping = resolve_wrapping_key(flags)?;
    let id = flags.get("id");
    let local = BackupClient::new(url, token)
        .pull_snapshot(id, Path::new(&out), &wrapping)
        .await?;
    println!("pulled + verified: {}", local.display());
    Ok(())
}

async fn cmd_verify(flags: &Flags) -> CmdResult {
    let dir = flags.req("snapshot-dir")?;
    let wrapping = resolve_wrapping_key(flags)?;
    let summary = snapshot::verify_snapshot(Path::new(&dir), &wrapping)?;
    println!(
        "snapshot {} verified: {} files, {} bytes, {} recording rows",
        summary.receipt.snapshot_id,
        summary.files_checked,
        summary.total_bytes,
        summary.receipt.recording_count
    );
    Ok(())
}

async fn cmd_restore(flags: &Flags) -> CmdResult {
    let dir = flags.req("snapshot-dir")?;
    let dest = flags.req("dest")?;
    let wrapping = resolve_wrapping_key(flags)?;
    let report = snapshot::restore_snapshot(Path::new(&dir), &wrapping, Path::new(&dest))?;
    println!(
        "restored {} → {} ({} files, db key recovered: {})",
        report.snapshot_id, dest, report.files_restored, report.db_key_recovered
    );
    println!("verify with: ferriscribe-backup drill --snapshot-dir {dir}");
    Ok(())
}

async fn cmd_drill(flags: &Flags) -> CmdResult {
    let wrapping = resolve_wrapping_key(flags)?;

    let snapshot_dir = if let Some(dir) = flags.get("snapshot-dir") {
        PathBuf::from(dir)
    } else if let Some(url) = flags.get("url") {
        // Drill the FULL path: pull the latest from the target, then drill.
        let token = flags.req("token")?;
        let staging = std::env::temp_dir().join(format!(
            "ferriscribe-drill-pull-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&staging)?;
        let local = BackupClient::new(url, token)
            .pull_snapshot(None, &staging, &wrapping)
            .await?;
        println!(
            "pulled latest from target for drilling: {}",
            local.display()
        );
        local
    } else {
        return Err(medical_backup::BackupError::Escrow(
            "drill needs --snapshot-dir or --url/--token".into(),
        ));
    };

    let outcome = drill::run_drill(&snapshot_dir, &wrapping);
    for check in &outcome.checks {
        println!("  ✓ {check}");
    }
    if outcome.passed {
        println!("DRILL PASSED for {}", outcome.snapshot_id);
        Ok(())
    } else {
        for failure in &outcome.failures {
            eprintln!("  ✗ {failure}");
        }
        Err(medical_backup::BackupError::Verification(
            "DRILL FAILED — this backup cannot be restored".into(),
        ))
    }
}

async fn cmd_serve(flags: &Flags) -> CmdResult {
    let root = flags.req("root")?;
    let bind = flags
        .get("bind")
        .unwrap_or("0.0.0.0:8741")
        .parse::<std::net::SocketAddr>()
        .map_err(|e| medical_backup::BackupError::Escrow(format!("--bind: {e}")))?;
    let append_token = std::env::var("FERRISCRIBE_BACKUP_APPEND_TOKEN").map_err(|_| {
        medical_backup::BackupError::Escrow("FERRISCRIBE_BACKUP_APPEND_TOKEN is required".into())
    })?;
    let admin_token = std::env::var("FERRISCRIBE_BACKUP_ADMIN_TOKEN").map_err(|_| {
        medical_backup::BackupError::Escrow(
            "FERRISCRIBE_BACKUP_ADMIN_TOKEN is required (target-side prune credential)".into(),
        )
    })?;
    std::fs::create_dir_all(&root)?;
    println!(
        "serving append-only backup agent on {bind} (root: {root}) — bind to a Tailscale IP, not 0.0.0.0, when possible"
    );
    agent::serve(
        AgentConfig {
            root: PathBuf::from(&root),
            append_token,
            admin_token,
        },
        bind,
    )
    .await?;
    Ok(())
}

fn cmd_install_schedule(flags: &Flags) -> CmdResult {
    let hour: u32 = flags
        .req("hour")?
        .parse()
        .map_err(|_| medical_backup::BackupError::Escrow("--hour must be 0-23".into()))?;
    let minute: u32 = flags
        .req("minute")?
        .parse()
        .map_err(|_| medical_backup::BackupError::Escrow("--minute must be 0-59".into()))?;
    let data_dir = default_data_dir();
    let cfg = schedule::ScheduleConfig {
        binary_path: std::env::current_exe()?,
        hour,
        minute,
        url: flags.get("url").unwrap_or("").to_string(),
        token: flags.get("token").unwrap_or("").to_string(),
        snapshots_dir: data_dir.join("backups"),
        log_dir: data_dir.join("logs"),
    };
    let path = schedule::install(&cfg)?;
    println!(
        "installed launchd agent ({}) → {}",
        schedule::LAUNCHD_LABEL,
        path.display()
    );
    println!(
        "fires daily at {hour:02}:{minute:02}; logs in {}",
        cfg.log_dir.display()
    );
    Ok(())
}

fn cmd_uninstall_schedule() -> CmdResult {
    schedule::uninstall()?;
    println!("removed launchd agent ({})", schedule::LAUNCHD_LABEL);
    Ok(())
}
