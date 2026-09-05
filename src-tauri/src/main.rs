// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Headless screenshot-OCR trigger (Omarchy-style compositor/global
    // binding): delegate to the ALREADY-RUNNING instance via the
    // single-instance plugin instead of booting a second app shell.
    // Parses BEFORE `run()` — keychain/SQLCipher/webview must never be
    // double-initialized, and the cold-start rule (no running instance →
    // notify + nonzero exit) must not launch a full GUI either.
    if std::env::args().any(|a| a == "--capture-ocr") {
        rust_medical_assistant_lib::delegate_capture_ocr();
    }
    rust_medical_assistant_lib::run();
}
