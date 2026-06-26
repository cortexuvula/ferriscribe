//! Tauri command exposing the bundled BC MSP ICD-9 code set to the
//! frontend for post-generation validation of SOAP-note codes.

use medical_core::error::AppResult;

/// Returns all 7,122 BC MSP ICD-9 diagnostic codes as a sorted vector.
///
/// The frontend caches this once and checks emitted SOAP codes against
/// it, flagging any code not on the list. Memoized on the Rust side via
/// [`medical_core::icd9`]'s `LazyLock`, so repeated invocations are cheap.
#[tauri::command]
pub async fn get_icd9_code_set() -> AppResult<Vec<String>> {
    Ok(medical_core::icd9::code_set().iter().cloned().collect())
}
