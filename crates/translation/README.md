# medical-translation

Text translation for clinician–patient conversations. Provides an AI-powered translation backend (wrapping any `AiProvider`) and a set of pre-built canned medical responses with hardcoded translations for common phrases.

## How It Fits

```
medical-core          ← TranslationProvider trait, Language type, AppResult
medical-translation ← this crate: AI translator, canned responses, session state
src-tauri             ← Tauri commands, session lifecycle, UI integration
```

Depends on `medical-core` for the `TranslationProvider` trait, `AiProvider` trait, completion types (`CompletionRequest`, `Message`, `Role`), and the shared error model. Used by `src-tauri` to power real-time translation during consultations.

## Module Map

| Module | Purpose |
|---|---|
| `lib.rs` | Crate root — re-exports submodules, defines `TranslationError` and `TranslationResult` |
| `session` | `TranslationSession`, `TranslationEntry`, `Speaker`, `TranslationMode` — conversation state |
| `canned_responses` | `CannedResponseSet`, `CannedResponse` — pre-translated medical phrases |
| `ai_translator` | `AiTranslationProvider` — implements `TranslationProvider` via any `AiProvider` |

## Key Types

### Provider Trait (`medical_core::traits::TranslationProvider`)

`AiTranslationProvider` implements the `TranslationProvider` trait from `medical-core`:

```rust
#[async_trait]
pub trait TranslationProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn supported_languages(&self) -> AppResult<Vec<Language>>;
    async fn translate(&self, text: &str, source_language: Option<&str>, target_language: &str) -> AppResult<String>;
    async fn detect_language(&self, text: &str) -> AppResult<String>;
}
```

### Session State

- **`TranslationSession`** — tracks a conversation between a provider (source language) and patient (target language), accumulating `TranslationEntry` records with timestamps and speaker identity.
- **`TranslationMode`** — `Bidirectional` (both directions active) or `OneWay` (source to target only).
- **`Speaker`** — `Provider` or `Patient`; determines translation direction when adding entries.

### Canned Responses

- **`CannedResponse`** — a single phrase (English + translations map) with an `id` and `category`.
- **`CannedResponseSet`** — a lookup collection. `default_medical()` returns 7 phrases across 4 categories (general, assessment, history, instructions) translated into es/fr/de/zh.

## How It Works

### Translation Request Flow

```
User utterance (text + speaker identity)
  │
  ▼
Check canned responses first           ← O(n) lookup by id, instant
  │
  ├─ hit → return pre-built translation (no AI call)
  │
  ▼ (miss)
AiTranslationProvider::translate()     ← sends prompt to AiProvider
  │  system: "You are a medical translator..."
  │  user:   "Translate from {src} to {tgt}... {text}"
  │  temperature: 0.1 (low — medical accuracy)
  ▼
Trimmed translation string
  │
  ▼
session.add_entry(original, translated, speaker)
  │
  ▼
TranslationEntry stored in history
```

### When Canned Responses Are Used vs AI

Canned responses are the fast path: when the UI presents a known phrase (e.g., "Where does it hurt?"), the app can look up the pre-translated text by response id and target language — no AI round-trip. This is both faster and more reliable for common clinical phrases.

AI translation handles everything else: free-form provider dictation, patient responses, and any text not covered by the canned set. The low temperature (0.1) keeps translations deterministic and faithful to medical terminology.

### Language Direction Logic

`TranslationSession::add_entry` automatically determines direction based on speaker:

- **Provider** utterances → `source_lang` to `target_lang` (e.g., en → es)
- **Patient** utterances → `target_lang` to `source_lang` (e.g., es → en)

This means the caller only needs to identify *who* spoke, not compute the direction.

### Language Detection

`AiTranslationProvider::detect_language` sends a minimal prompt asking for a BCP-47 code. Temperature is 0.0 for maximum determinism. If the model returns empty/whitespace, it defaults to `"en"`.

## Examples

### Creating a Session

```rust
use medical_translation::session::{TranslationSession, TranslationMode, Speaker};

let mut session = TranslationSession::new("en", "es", TranslationMode::Bidirectional);

session.add_entry("Where does it hurt?", "¿Dónde le duele?", Speaker::Provider);
session.add_entry("Me duele la cabeza", "My head hurts", Speaker::Patient);

assert_eq!(session.entry_count(), 2);
println!("{}", session.export_text());
// [14:30:01] Provider (en→es): Where does it hurt?
//   → ¿Dónde le duele?
// [14:30:05] Patient (es→en): Me duele la cabeza
//   → My head hurts
```

### Using Canned Responses

```rust
use medical_translation::canned_responses::CannedResponseSet;

let set = CannedResponseSet::default_medical();

// Look up a specific phrase
if let Some(spanish) = set.get_translation("assessment_pain_location", "es") {
    assert_eq!(spanish, "¿Dónde le duele?");
}

// Browse by category
for response in set.by_category("history") {
    println!("{}: {}", response.id, response.text_en);
}
```

### AI Translation

```rust
use std::sync::Arc;
use medical_translation::ai_translator::AiTranslationProvider;
use medical_core::traits::TranslationProvider;

let ai_provider: Arc<dyn medical_core::traits::AiProvider> = /* your provider */;
let translator = AiTranslationProvider::new(ai_provider);

let result = translator.translate("Hello", Some("en"), "es").await?;
// → "Hola"

let lang = translator.detect_language("Bonjour").await?;
// → "fr"
```

## Gotchas

### Language Pair Coverage Differs

Canned responses only cover **en → {es, fr, de, zh}** (7 phrases, 4 target languages). AI translation advertises **12 languages** (en, es, fr, de, zh, ja, ko, pt, ar, hi, ru, it). If you add a new language to `supported_languages`, it won't have canned-response fallback — all translation will go through the AI path.

### Canned Responses Are English-Centric

`CannedResponse.text_en` is the canonical form. There is no support for looking up a canned response by non-English text. If you need to match a Spanish phrase to its canned entry, you'd need to search `translations` values manually.

### No Caching on AI Translations

Each call to `AiTranslationProvider::translate` fires a fresh completion request. Identical texts translated twice produce two AI calls. If caching is needed, it must be layered above this crate (e.g., in `src-tauri`).

### Temperature Matters

Translation uses `temperature: 0.1` (low but not zero) — this gives the model slight flexibility for natural phrasing while keeping medical terms accurate. Language detection uses `0.0` for strict determinism. Changing these values can affect translation quality, especially for ambiguous medical abbreviations.

### Export Format Is Not Stable

`TranslationSession::export_text()` returns a human-readable string. The format is meant for display/copy-paste, not machine parsing. Don't rely on the exact format in serialization or tests beyond substring checks.
