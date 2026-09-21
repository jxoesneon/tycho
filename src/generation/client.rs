//! Streaming chat completion client for conversational queries.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResponse {
    pub content: String,
    pub model: String,
}

/// Generation limits applied to every request — temperature plus the
/// output token cap (persona budget capped by the configured maximum).
#[derive(Debug, Clone, Copy)]
pub struct GenerateOptions {
    pub temperature: f32,
    pub max_tokens: u32,
}

pub struct GenerationClient {
    pub model: String,
    pub endpoint: String,
    pub api_key: Option<String>,
    client: reqwest::Client,
}

impl GenerationClient {
    pub fn new(
        model: impl Into<String>,
        endpoint: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("HTTP client with only a timeout cannot fail to build");
        Self {
            model: model.into(),
            endpoint: endpoint.into(),
            api_key,
            client,
        }
    }

    pub async fn generate(
        &self,
        prompt: &str,
        system_instruction: &str,
    ) -> Result<GenerationResponse> {
        self.generate_conversation(prompt, system_instruction, &[])
            .await
    }

    fn build_messages(
        prompt: &str,
        system_instruction: &str,
        history: &[ChatMessage],
    ) -> Vec<serde_json::Value> {
        let mut messages = Vec::with_capacity(history.len() + 2);
        messages.push(serde_json::json!({ "role": "system", "content": system_instruction }));
        for m in history {
            messages.push(serde_json::json!({ "role": m.role, "content": m.content }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));
        messages
    }

    async fn send(&self, body: &serde_json::Value) -> Result<reqwest::Response> {
        let mut req = self.client.post(&self.endpoint).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Inference(format!("request to {} failed: {}", self.endpoint, e)))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(Error::Inference(format!(
                "endpoint returned {}: {}",
                status,
                detail.chars().take(200).collect::<String>()
            )));
        }
        Ok(resp)
    }

    /// Chat completion with prior turns threaded in as proper
    /// role-tagged messages — the model sees actual conversation
    /// history, not a flattened transcript. `opts` bounds the
    /// generation so a chatty model cannot ramble past the persona's
    /// spoken-answer budget.
    pub async fn generate_conversation(
        &self,
        prompt: &str,
        system_instruction: &str,
        history: &[ChatMessage],
    ) -> Result<GenerationResponse> {
        self.generate_conversation_opts(prompt, system_instruction, history, None)
            .await
    }

    pub async fn generate_conversation_opts(
        &self,
        prompt: &str,
        system_instruction: &str,
        history: &[ChatMessage],
        opts: Option<GenerateOptions>,
    ) -> Result<GenerationResponse> {
        if prompt.is_empty() {
            return Err(Error::Inference("Empty user prompt provided".to_string()));
        }

        let messages = Self::build_messages(prompt, system_instruction, history);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
        });
        if let Some(o) = opts {
            body["temperature"] = serde_json::json!(o.temperature);
            body["max_tokens"] = serde_json::json!(o.max_tokens);
        }

        let resp = self.send(&body).await?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Inference(format!("malformed response body: {}", e)))?;

        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Inference("response missing choices[0].message.content".to_string())
            })?
            .to_string();

        Ok(GenerationResponse {
            content,
            model: self.model.clone(),
        })
    }

    /// Streaming variant: SSE deltas arrive on the returned channel as
    /// the model produces them, so the caller can start TTS on the
    /// first complete sentence instead of waiting for the whole
    /// response. The channel closes at `[DONE]`/EOF; an `Err` item
    /// signals failure (caller falls back to the non-streaming path).
    pub async fn generate_conversation_stream(
        &self,
        prompt: &str,
        system_instruction: &str,
        history: &[ChatMessage],
        opts: GenerateOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<Result<String>>> {
        if prompt.is_empty() {
            return Err(Error::Inference("Empty user prompt provided".to_string()));
        }

        let messages = Self::build_messages(prompt, system_instruction, history);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "temperature": opts.temperature,
            "max_tokens": opts.max_tokens,
        });

        let mut resp = self.send(&body).await?;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String>>(16);
        tokio::spawn(async move {
            let mut buf = String::new();
            loop {
                match resp.chunk().await {
                    Ok(Some(bytes)) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // SSE frames are `data: {json}` lines.
                        while let Some(nl) = buf.find('\n') {
                            let line = buf[..nl].trim().to_string();
                            buf.drain(..=nl);
                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data == "[DONE]" {
                                return;
                            }
                            match serde_json::from_str::<serde_json::Value>(data) {
                                Ok(j) => {
                                    if let Some(d) = j
                                        .pointer("/choices/0/delta/content")
                                        .and_then(|v| v.as_str())
                                    {
                                        if tx.send(Ok(d.to_string())).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(Err(Error::Inference(format!(
                                            "malformed stream chunk: {e}"
                                        ))))
                                        .await;
                                    return;
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // EOF without [DONE] — channel close tells the
                        // caller the stream ended; whatever arrived is
                        // still usable.
                        return;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(Error::Inference(format!("stream failed: {e}"))))
                            .await;
                        return;
                    }
                }
            }
        });
        Ok(rx)
    }
}
