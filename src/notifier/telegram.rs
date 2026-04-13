//! Telegram notification backend using the Bot API via [`reqwest`].
//!
//! Sends build lifecycle messages to a Telegram chat.  Messages are
//! truncated to 4096 characters (Telegram limit).

use std::time::Duration;

use crate::error::NotifierError;
use crate::notifier::Notifier;

/// Maximum Telegram message length (characters).
const MAX_MESSAGE_LEN: usize = 4096;

/// Sends build notifications to a Telegram chat via the Bot API.
///
/// The `Debug` impl is manually implemented to redact the token
/// embedded in `api_url`.
#[derive(Clone)]
pub struct TelegramNotifier {
    client: reqwest::Client,
    /// Bot API URL: `https://api.telegram.org/bot{token}/sendMessage`
    api_url: String,
    /// Target chat ID.
    chat_id: String,
}

impl std::fmt::Debug for TelegramNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramNotifier")
            .field(
                "api_url",
                &"https://api.telegram.org/bot<REDACTED>/sendMessage",
            )
            .field("chat_id", &self.chat_id)
            .finish()
    }
}

impl TelegramNotifier {
    /// Create a new Telegram notifier.
    ///
    /// # Arguments
    ///
    /// * `token` - Bot API token (from `TELEGRAM_TOKEN` env var)
    /// * `chat_id` - Target chat ID (from `TELEGRAM_CHAT_ID` env var)
    ///
    /// # Errors
    ///
    /// Returns [`NotifierError::Config`] if the token or chat ID is
    /// empty.
    pub fn new(token: &str, chat_id: &str) -> Result<Self, NotifierError> {
        if token.is_empty() {
            return Err(NotifierError::Config("TELEGRAM_TOKEN is empty".to_owned()));
        }
        if chat_id.is_empty() {
            return Err(NotifierError::Config(
                "TELEGRAM_CHAT_ID is empty".to_owned(),
            ));
        }

        Ok(Self {
            client: reqwest::Client::new(),
            api_url: format!("https://api.telegram.org/bot{token}/sendMessage"),
            chat_id: chat_id.to_owned(),
        })
    }

    /// Send a text message to the configured chat.
    async fn send(&self, text: &str) -> Result<(), NotifierError> {
        let truncated = if text.len() > MAX_MESSAGE_LEN {
            let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
            &text[..boundary]
        } else {
            text
        };

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": truncated,
            "parse_mode": "Markdown",
        });

        let resp = self
            .client
            .post(&self.api_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| NotifierError::Send(format!("HTTP: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".to_owned());
            return Err(NotifierError::Send(format!(
                "Telegram API {status}: {body}"
            )));
        }

        Ok(())
    }
}

impl Notifier for TelegramNotifier {
    /// Notify that a build has started.
    async fn notify_build_start(&self, recipe: &str, tag: &str) -> Result<(), NotifierError> {
        let msg = format!("*Build started*\nRecipe: `{recipe}`\nTag: `{tag}`");
        self.send(&msg).await
    }

    /// Notify that a build succeeded.
    async fn notify_build_success(
        &self,
        recipe: &str,
        tag: &str,
        duration: Duration,
    ) -> Result<(), NotifierError> {
        let secs = duration.as_secs();
        let msg = format!(
            "*Build succeeded*\nRecipe: `{recipe}`\nTag: `{tag}`\n\
             Duration: {secs}s"
        );
        self.send(&msg).await
    }

    /// Notify that a build failed.
    async fn notify_build_failure(
        &self,
        recipe: &str,
        tag: &str,
        error: &str,
    ) -> Result<(), NotifierError> {
        let msg = format!(
            "*Build failed*\nRecipe: `{recipe}`\nTag: `{tag}`\n\
             Error: ```\n{error}\n```"
        );
        self.send(&msg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_empty_token() {
        let err = TelegramNotifier::new("", "123").unwrap_err();
        assert!(matches!(err, NotifierError::Config(_)));
    }

    #[test]
    fn new_validates_empty_chat_id() {
        let err = TelegramNotifier::new("tok", "").unwrap_err();
        assert!(matches!(err, NotifierError::Config(_)));
    }

    #[test]
    fn new_succeeds() {
        let n = TelegramNotifier::new("tok123", "456").expect("ok");
        assert!(n.api_url.contains("tok123"));
        assert_eq!(n.chat_id, "456");
    }

    #[test]
    fn telegram_notifier_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TelegramNotifier>();
    }

    #[test]
    fn truncate_respects_char_boundary() {
        // Build a string that is exactly MAX_MESSAGE_LEN + 2 bytes long,
        // with a multi-byte character straddling the boundary.
        // '\u{00e9}' (e-acute) is 2 bytes in UTF-8.
        let filler_len = MAX_MESSAGE_LEN - 1;
        let mut text = "a".repeat(filler_len);
        // This 2-byte char starts at byte filler_len, so byte
        // MAX_MESSAGE_LEN falls in the middle of it.
        text.push('\u{00e9}');
        text.push('z');
        assert!(text.len() > MAX_MESSAGE_LEN);

        // Replicate the truncation logic from `send`.
        let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
        let truncated = &text[..boundary];

        // Must be valid UTF-8 and at most MAX_MESSAGE_LEN bytes.
        assert!(truncated.len() <= MAX_MESSAGE_LEN);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn new_builds_correct_api_url() {
        let n = TelegramNotifier::new("mytoken123", "chat42").expect("valid inputs");
        assert_eq!(
            n.api_url,
            "https://api.telegram.org/botmytoken123/sendMessage",
        );
    }

    #[test]
    fn new_stores_chat_id() {
        let n = TelegramNotifier::new("tok", "chat999").expect("valid inputs");
        assert_eq!(n.chat_id, "chat999");
    }

    #[test]
    fn new_empty_token_error_message_mentions_telegram_token() {
        let err = TelegramNotifier::new("", "123").unwrap_err();
        match err {
            NotifierError::Config(msg) => {
                assert!(
                    msg.contains("TELEGRAM_TOKEN"),
                    "error should mention TELEGRAM_TOKEN, got: {msg}",
                );
            },
            other => panic!("expected Config error, got: {other:?}"),
        }
    }

    #[test]
    fn new_empty_chat_id_error_message_mentions_telegram_chat_id() {
        let err = TelegramNotifier::new("tok", "").unwrap_err();
        match err {
            NotifierError::Config(msg) => {
                assert!(
                    msg.contains("TELEGRAM_CHAT_ID"),
                    "error should mention TELEGRAM_CHAT_ID, got: {msg}",
                );
            },
            other => panic!("expected Config error, got: {other:?}"),
        }
    }

    #[test]
    fn debug_redacts_token() {
        let n = TelegramNotifier::new("secret_tok", "chat1").expect("valid inputs");
        let debug = format!("{n:?}");
        assert!(
            !debug.contains("secret_tok"),
            "Debug output must not contain the real token",
        );
        assert!(
            debug.contains("REDACTED"),
            "Debug output should contain REDACTED placeholder",
        );
        assert!(
            debug.contains("chat1"),
            "Debug output should show the chat_id",
        );
    }

    #[test]
    fn notify_build_start_message_format() {
        let recipe = "Cosmos";
        let tag = "v1.0.0";
        let msg = format!("*Build started*\nRecipe: `{recipe}`\nTag: `{tag}`");
        assert!(msg.contains("*Build started*"));
        assert!(msg.contains("Recipe: `Cosmos`"));
        assert!(msg.contains("Tag: `v1.0.0`"));
    }

    #[test]
    fn notify_build_success_message_format() {
        let recipe = "Osmosis";
        let tag = "v2.0.0";
        let duration = Duration::from_secs(142);
        let secs = duration.as_secs();
        let msg = format!(
            "*Build succeeded*\nRecipe: `{recipe}`\nTag: `{tag}`\n\
             Duration: {secs}s"
        );
        assert!(msg.contains("*Build succeeded*"));
        assert!(msg.contains("Recipe: `Osmosis`"));
        assert!(msg.contains("Tag: `v2.0.0`"));
        assert!(msg.contains("Duration: 142s"));
    }

    #[test]
    fn notify_build_failure_message_format() {
        let recipe = "Juno";
        let tag = "v3.0.0";
        let error = "OOM killed";
        let msg = format!(
            "*Build failed*\nRecipe: `{recipe}`\nTag: `{tag}`\n\
             Error: ```\n{error}\n```"
        );
        assert!(msg.contains("*Build failed*"));
        assert!(msg.contains("Recipe: `Juno`"));
        assert!(msg.contains("Tag: `v3.0.0`"));
        assert!(msg.contains("Error: ```\nOOM killed\n```"));
    }

    #[test]
    fn notify_build_success_duration_zero() {
        let duration = Duration::from_secs(0);
        let secs = duration.as_secs();
        let msg = format!("*Build succeeded*\nRecipe: `r`\nTag: `t`\nDuration: {secs}s");
        assert!(msg.contains("Duration: 0s"));
    }

    #[test]
    fn notify_build_failure_error_with_special_chars() {
        let error = "line1\nline2\ttab";
        let msg = format!(
            "*Build failed*\nRecipe: `r`\nTag: `t`\n\
             Error: ```\n{error}\n```"
        );
        assert!(msg.contains("line1\nline2\ttab"));
    }

    #[test]
    fn truncation_not_applied_when_under_limit() {
        let text = "a".repeat(MAX_MESSAGE_LEN);
        // Text is exactly at the limit, no truncation needed.
        let truncated = if text.len() > MAX_MESSAGE_LEN {
            let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
            &text[..boundary]
        } else {
            &text
        };
        assert_eq!(truncated.len(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn truncation_applied_when_over_limit() {
        let text = "b".repeat(MAX_MESSAGE_LEN + 100);
        let truncated = if text.len() > MAX_MESSAGE_LEN {
            let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
            &text[..boundary]
        } else {
            &text
        };
        assert_eq!(truncated.len(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn truncation_with_3_byte_char_at_boundary() {
        // '\u{2603}' (snowman) is 3 bytes in UTF-8.
        let filler_len = MAX_MESSAGE_LEN - 2;
        let mut text = "x".repeat(filler_len);
        text.push('\u{2603}');
        text.push('z');
        assert!(text.len() > MAX_MESSAGE_LEN);

        let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
        let truncated = &text[..boundary];
        assert!(truncated.len() <= MAX_MESSAGE_LEN);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn truncation_with_4_byte_char_at_boundary() {
        // '\u{1F600}' (grinning face) is 4 bytes in UTF-8.
        let filler_len = MAX_MESSAGE_LEN - 3;
        let mut text = "x".repeat(filler_len);
        text.push('\u{1F600}');
        text.push('z');
        assert!(text.len() > MAX_MESSAGE_LEN);

        let boundary = text.floor_char_boundary(MAX_MESSAGE_LEN);
        let truncated = &text[..boundary];
        assert!(truncated.len() <= MAX_MESSAGE_LEN);
        assert!(truncated.is_char_boundary(truncated.len()));
    }
}
