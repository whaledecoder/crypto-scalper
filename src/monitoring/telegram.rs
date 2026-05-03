//! Telegram Bot API notifier.

use crate::errors::Result;
use reqwest::Client;
use tracing::warn;

pub struct TelegramNotifier {
    client: Client,
    token: String,
    chat_id: String,
    enabled: bool,
}

impl TelegramNotifier {
    pub fn new(token: String, chat_id: String) -> Self {
        let enabled = !token.is_empty() && !chat_id.is_empty();
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            token,
            chat_id,
            enabled,
        }
    }

    pub async fn send(&self, text: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
            "disable_web_page_preview": true,
            "parse_mode": "HTML",
        });
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            warn!(status = %resp.status(), "telegram send failed");
        }
        Ok(())
    }
}
