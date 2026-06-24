//! AI-powered translation using any [`AiProvider`] backend.
//!
//! Wraps a generic AI completion provider so it can be used as a
//! [`TranslationProvider`], enabling medical text translation through
//! large-language-model prompts.

use std::sync::Arc;

use async_trait::async_trait;

use medical_core::error::AppResult;
use medical_core::traits::translation::Language;
use medical_core::traits::{AiProvider, TranslationProvider};
use medical_core::types::{CompletionRequest, Message, MessageContent, Role};

/// A [`TranslationProvider`] that delegates to an [`AiProvider`].
///
/// Translation and language detection are handled via carefully constructed
/// prompts that preserve medical-domain accuracy.
pub struct AiTranslationProvider {
    provider: Arc<dyn AiProvider>,
}

impl AiTranslationProvider {
    /// Create a new AI-backed translation provider.
    ///
    /// The given `provider` will receive completion requests whose prompts
    /// instruct the model to translate or detect languages.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl TranslationProvider for AiTranslationProvider {
    fn name(&self) -> &str {
        "ai"
    }

    async fn supported_languages(&self) -> AppResult<Vec<Language>> {
        Ok(vec![
            Language {
                code: "en".into(),
                name: "English".into(),
            },
            Language {
                code: "es".into(),
                name: "Spanish".into(),
            },
            Language {
                code: "fr".into(),
                name: "French".into(),
            },
            Language {
                code: "de".into(),
                name: "German".into(),
            },
            Language {
                code: "zh".into(),
                name: "Chinese".into(),
            },
            Language {
                code: "ja".into(),
                name: "Japanese".into(),
            },
            Language {
                code: "ko".into(),
                name: "Korean".into(),
            },
            Language {
                code: "pt".into(),
                name: "Portuguese".into(),
            },
            Language {
                code: "ar".into(),
                name: "Arabic".into(),
            },
            Language {
                code: "hi".into(),
                name: "Hindi".into(),
            },
            Language {
                code: "ru".into(),
                name: "Russian".into(),
            },
            Language {
                code: "it".into(),
                name: "Italian".into(),
            },
        ])
    }

    async fn translate(
        &self,
        text: &str,
        source_language: Option<&str>,
        target_language: &str,
    ) -> AppResult<String> {
        let source_desc = source_language.unwrap_or("the source language");

        let request = CompletionRequest {
            model: String::new(), // provider will use its default
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "Translate the following medical text from {source_desc} to {target_language}. \
                     Preserve medical terminology accuracy. Return ONLY the translation:\n\n{text}"
                )),
                tool_calls: vec![],
            }],
            temperature: Some(0.1),
            max_tokens: Some(4096),
            system_prompt: Some(
                "You are a medical translator. Translate accurately, preserving clinical terminology."
                    .into(),
            ),
        };

        let response = self.provider.complete(request).await?;
        Ok(response.content.trim().to_string())
    }

    async fn detect_language(&self, text: &str) -> AppResult<String> {
        let request = CompletionRequest {
            model: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text(format!(
                    "Detect the language of this text. Return ONLY the BCP-47 language \
                     code (e.g. 'en', 'es'):\n\n{text}"
                )),
                tool_calls: vec![],
            }],
            temperature: Some(0.0),
            max_tokens: Some(10),
            system_prompt: None,
        };

        let response = self.provider.complete(request).await?;
        let code = response.content.trim().to_lowercase();
        if code.is_empty() {
            Ok("en".into())
        } else {
            Ok(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_core::Stream;
    use medical_core::types::{
        CompletionResponse, ModelInfo, StreamChunk, ToolCompletionResponse, ToolDef, UsageInfo,
    };
    use std::sync::Mutex;

    /// A mock AI provider that records requests and returns canned responses.
    struct MockAiProvider {
        responses: Mutex<Vec<String>>,
        captured: Mutex<Vec<CompletionRequest>>,
    }

    impl MockAiProvider {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(String::from).collect()),
                captured: Mutex::new(Vec::new()),
            }
        }

        fn captured_requests(&self) -> Vec<CompletionRequest> {
            self.captured.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AiProvider for MockAiProvider {
        fn name(&self) -> &str {
            "mock"
        }

        async fn available_models(&self) -> AppResult<Vec<ModelInfo>> {
            Ok(vec![])
        }

        async fn complete(&self, request: CompletionRequest) -> AppResult<CompletionResponse> {
            self.captured.lock().unwrap().push(request);
            let content = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| "mock response".into());
            Ok(CompletionResponse {
                content,
                model: "mock-model".into(),
                usage: UsageInfo::default(),
                tool_calls: vec![],
            })
        }

        async fn complete_stream(
            &self,
            _request: CompletionRequest,
        ) -> AppResult<Box<dyn Stream<Item = AppResult<StreamChunk>> + Send + Unpin>> {
            unimplemented!("not used in tests")
        }

        async fn complete_with_tools(
            &self,
            _request: CompletionRequest,
            _tools: Vec<ToolDef>,
        ) -> AppResult<ToolCompletionResponse> {
            unimplemented!("not used in tests")
        }
    }

    #[tokio::test]
    async fn translate_constructs_correct_prompt() {
        let mock = Arc::new(MockAiProvider::new(vec!["Hola mundo"]));
        let translator = AiTranslationProvider::new(mock.clone());

        let result = translator
            .translate("Hello world", Some("en"), "es")
            .await
            .unwrap();

        assert_eq!(result, "Hola mundo");

        let requests = mock.captured_requests();
        assert_eq!(requests.len(), 1);

        let req = &requests[0];
        assert_eq!(req.temperature, Some(0.1));
        assert_eq!(req.max_tokens, Some(4096));
        assert!(req.system_prompt.is_some());
        assert!(
            req.system_prompt
                .as_ref()
                .unwrap()
                .contains("medical translator")
        );

        // Verify the user message contains the expected elements.
        if let MessageContent::Text(ref text) = req.messages[0].content {
            assert!(text.contains("en"));
            assert!(text.contains("es"));
            assert!(text.contains("Hello world"));
        } else {
            panic!("Expected Text message content");
        }
    }

    #[tokio::test]
    async fn translate_auto_detect_source() {
        let mock = Arc::new(MockAiProvider::new(vec!["Translated text"]));
        let translator = AiTranslationProvider::new(mock.clone());

        let result = translator.translate("Some text", None, "fr").await.unwrap();
        assert_eq!(result, "Translated text");

        let requests = mock.captured_requests();
        if let MessageContent::Text(ref text) = requests[0].messages[0].content {
            assert!(text.contains("the source language"));
            assert!(text.contains("fr"));
        } else {
            panic!("Expected Text message content");
        }
    }

    #[tokio::test]
    async fn detect_language_returns_code() {
        let mock = Arc::new(MockAiProvider::new(vec!["  ES  "]));
        let translator = AiTranslationProvider::new(mock.clone());

        let code = translator
            .detect_language("Me duele la cabeza")
            .await
            .unwrap();
        assert_eq!(code, "es");
    }

    #[tokio::test]
    async fn detect_language_defaults_to_en_on_empty() {
        let mock = Arc::new(MockAiProvider::new(vec!["  "]));
        let translator = AiTranslationProvider::new(mock.clone());

        let code = translator.detect_language("some text").await.unwrap();
        assert_eq!(code, "en");
    }

    #[tokio::test]
    async fn supported_languages_returns_expected_set() {
        let mock = Arc::new(MockAiProvider::new(vec![]));
        let translator = AiTranslationProvider::new(mock);

        let langs = translator.supported_languages().await.unwrap();
        assert!(langs.len() >= 12);

        let codes: Vec<&str> = langs.iter().map(|l| l.code.as_str()).collect();
        assert!(codes.contains(&"en"));
        assert!(codes.contains(&"es"));
        assert!(codes.contains(&"zh"));
        assert!(codes.contains(&"ar"));
    }

    #[test]
    fn provider_name_is_ai() {
        let mock = Arc::new(MockAiProvider::new(vec![]));
        let translator = AiTranslationProvider::new(mock);
        assert_eq!(translator.name(), "ai");
    }
}
