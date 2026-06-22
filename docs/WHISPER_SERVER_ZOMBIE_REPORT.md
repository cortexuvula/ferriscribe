# whisper-server Zombie Process Report

**Date:** June 16, 2026  
**Repo:** rustMedicalAssistant (FerriScribe)  
**File:** `crates/sharing/src/whisper_supervisor.rs` + `src-tauri/src/commands/sharing/lifecycle.rs`

---

## Problem

Three `whisper-server` processes were observed running simultaneously on the Mac Studio, all from FerriScribe, all attempting to bind to port 8080:

| PID | Started | Status |
|-----|---------|--------|
| 1609 | 10:38 AM | Stale zombie (2.0 GB RAM) |
| 31028 | 12:14 PM | Stale zombie (2.0 GB RAM) |
| 58039 | 1:29 PM | Active (holds port 8080) |

Each instance loads `ggml-large-v3-turbo.bin` (~2 GB). The two stale processes consumed ~4 GB of RAM while contributing nothing.

---

## Root Cause Analysis

### 1. Auto-resume spawns without checking for existing instances

`lib.rs:188-206` — On app startup, if `sharing-server.json` exists, the auto-resume hook calls `start_sharing_inner()` unconditionally:

```rust
// lib.rs:194
tauri::async_runtime::spawn(async move {
    let state = app_handle.state::<crate::state::AppState>();
    if let Err(e) = crate::commands::sharing::start_sharing_inner(
        &state, cfg.friendly_name, Some(app_handle.clone()),
    ).await {
        tracing::warn!(error = %e, "auto-resume sharing failed");
    }
});
```

This calls `WhisperSupervisor::start()` → `spawn_once_at()` which spawns a new `whisper-server` without checking if one is already running on port 8080.

### 2. No PID file or port check before spawn

`whisper_supervisor.rs:301-347` — `spawn_once_at()` directly calls `cmd.spawn()` without:
- Checking if port 8080 is already bound
- Writing/reading a PID file to detect stale instances
- Looking for existing `whisper-server` processes

```rust
async fn spawn_once_at(&self, bin: &Path) -> Result<Child> {
    let mut cmd = Command::new(bin);
    cmd.arg("--host").arg("127.0.0.1")
       .arg("--port").arg(self.port.to_string())
       .arg("-m").arg(&self.model_path)
       .arg("--inference-path").arg("/v1/audio/transcriptions")
       .stdout(Stdio::null())
       .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;  // ← no port check, no PID check
    // ...
}
```

When port 8080 is already occupied, the new `whisper-server` process either:
- Fails to bind and exits (triggering the supervisor's crash-restart loop with exponential backoff)
- Binds to a different port silently (unlikely with whisper.cpp)

Either way, the **original process keeps running** and a new one is spawned on each restart attempt.

### 3. `stop()` only kills the tracked child

`whisper_supervisor.rs:354-367` — `stop()` only kills the process stored in `self.child`:

```rust
pub async fn stop(&self) {
    self.stopped.store(true, Ordering::Relaxed);
    self.stop.notify_waiters();
    if let Some(mut c) = self.child.lock().await.take() {
        let _ = c.kill().await;  // ← only kills the CURRENT child
    }
    // ...
}
```

If the app crashes or is force-killed without calling `stop_sharing()`, the whisper-server child process continues running as an orphan. On next app launch, the auto-resume hook spawns a new one alongside the orphan.

### 4. No `Drop` implementation for `WhisperSupervisor`

There is no `impl Drop for WhisperSupervisor`. When the `SharingService` is dropped (e.g., `stop_sharing` takes it from the `Option` via `state.sharing.write().await.take()`), the child process handle is dropped without killing it. The `stop()` method must be explicitly called.

### 5. `stop_sharing` is not called on app close

`lifecycle.rs:167-223` — `stop_sharing` is a Tauri command that's only invoked when the user explicitly clicks "Stop Sharing" in the UI. There is **no automatic cleanup on app window close or quit**. The `SharingService` is simply dropped when the `AppState` is destroyed, leaving the child process running.

---

## Proposed Fixes

### Priority 1: Kill existing whisper-server before spawning

In `spawn_once_at()`, add a port-check + existing-process-kill step:

```rust
async fn spawn_once_at(&self, bin: &Path) -> Result<Child> {
    // Kill any existing whisper-server on our port
    Self::kill_existing_on_port(self.port).await;
    
    let mut cmd = Command::new(bin);
    // ... existing args ...
    let mut child = cmd.spawn()?;
    // ...
}

async fn kill_existing_on_port(port: u16) {
    // Use lsof or netstat to find PIDs on the port
    if let Ok(output) = tokio::process::Command::new("lsof")
        .args(["-ti", &format!(":{}", port)])
        .output()
        .await
    {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid_str in pids.lines() {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                let _ = tokio::process::Command::new("kill")
                    .arg(pid.to_string())
                    .output()
                    .await;
            }
        }
    }
}
```

### Priority 2: Add PID file tracking

Write the child PID to a file in `binary_dir` on spawn, check and clean up on start:

```rust
fn pid_file_path(&self) -> PathBuf {
    self.binary_dir.join("whisper-server.pid")
}

fn write_pid(&self, pid: u32) {
    let _ = std::fs::write(self.pid_file_path(), pid.to_string());
}

fn kill_stale_pid(&self) {
    if let Ok(pid_str) = std::fs::read_to_string(self.pid_file_path()) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            // Check if process still exists
            unsafe { libc::kill(pid as i32, 0) }; // SIGEXIST check
            // Kill if stale
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .output();
        }
        let _ = std::fs::remove_file(self.pid_file_path());
    }
}
```

### Priority 3: Add `Drop` implementation

```rust
impl Drop for WhisperSupervisor {
    fn drop(&mut self) {
        // Best-effort kill of the child process.
        // We can't use async in Drop, so use std::process.
        if let Some(mut c) = self.child.blocking_lock().take() {
            let _ = c.kill();
        }
    }
}
```

### Priority 4: Auto-stop on app close

In `lib.rs`, register a window close handler that calls `stop_sharing`:

```rust
// In the setup() closure
let app_handle = app.handle().clone();
app.on_window_event(move |event| {
    if let tauri::WindowEvent::CloseRequested { .. } = event.event() {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let state = handle.state::<AppState>();
            let _ = crate::commands::sharing::stop_sharing_inner(&state).await;
        });
    }
});
```

### Priority 5: Health check before spawn

Add a readiness probe before spawning — if port 8080 already has a healthy whisper-server, skip the spawn:

```rust
async fn is_port_alive(port: u16) -> bool {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/health", port))
        .timeout(Duration::from_secs(1))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
```

---

## Impact

- **Memory waste:** ~2 GB per zombie process (confirmed 4 GB wasted on 2 stale instances)
- **Port conflict:** New whisper-server instances fail to bind, triggering crash-restart loops with exponential backoff
- **Silent degradation:** The supervisor's backoff loop (1s → 60s cap) means the app appears to work (the original process handles requests) but wastes resources and creates confusing behavior

---

## Workaround (Immediate)

Until the code is fixed, kill stale processes manually:

```bash
pkill -f "whisper-server.*rust-medical-assistant"
# Or target specific PIDs
kill 1609 31028
```

Or add a pre-launch cleanup script to the app bundle.
