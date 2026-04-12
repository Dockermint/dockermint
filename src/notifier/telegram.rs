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
            &text[..MAX_MESSAGE_LEN]
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
}
