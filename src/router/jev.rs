//! Semantic intent matching and fallback routing over an OpenAI-compatible
//! chat endpoint.

use super::laya::LayaDecision;
use crate::error::{Error, Result};

pub struct JevSemanticRouter {
    pub api_key: Option<String>,
    pub endpoint: String,
    pub model: String,
    client: reqwest::Client,
}

impl JevSemanticRouter {
    pub fn new(
        api_key: Option<String>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            // Routing is on the critical path — a slow endpoint must
            // degrade to deliberative routing quickly, not stall the
            // turn for half a minute.
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("HTTP client with only a timeout cannot fail to build");
        Self {
            api_key,
            endpoint: endpoint.into(),
            model: model.into(),
            client,
        }
    }

    /// Asks the configured endpoint to pick the best option for `query` and
    /// parses the JSON verdict. Returns `Err` when the endpoint is
    /// unreachable or replies unintelligibly so the caller can fall back to
    /// deliberative routing.
    pub async fn route(&self, query: &str, options: &[&str]) -> Result<LayaDecision> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a semantic intent router. Pick the single best option from the provided list for the user's request. Respond with ONLY a JSON object {\"option\": \"<option>\", \"confidence\": <0.0-1.0>}."
                },
                {
                    "role": "user",
                    "content": format!("Options: {}\nRequest: {}", options.join(", "), query)
                },
            ],
            "stream": false,
            // A verdict is ~15 tokens of JSON; capping generation keeps
            // a rambling model from stalling the routing decision.
            "max_tokens": 48,
            "temperature": 0.1,
        });

        let mut req = self.client.post(&self.endpoint).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Routing(format!("jev request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(Error::Routing(format!(
                "jev endpoint returned {}",
                resp.status()
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Routing(format!("jev malformed response body: {}", e)))?;

        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Routing("jev response missing choices[0].message.content".to_string())
            })?;

        // The model may wrap the verdict in prose; extract the JSON object.
        let start = content
            .find('{')
            .ok_or_else(|| Error::Routing("jev content contains no JSON".to_string()))?;
        let end = content
            .rfind('}')
            .ok_or_else(|| Error::Routing("jev content contains no JSON".to_string()))?;
        let verdict: serde_json::Value = serde_json::from_str(&content[start..=end])
            .map_err(|e| Error::Routing(format!("jev verdict is not valid JSON: {}", e)))?;

        let option = verdict
            .get("option")
            .and_then(|v| v.as_str())
            .unwrap_or("general_query");
        let confidence = verdict
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;

        if options.contains(&option) {
            Ok(LayaDecision {
                best_option: option.to_string(),
                confidence: confidence.clamp(0.0, 1.0),
            })
        } else {
            Ok(LayaDecision {
                best_option: "general_query".to_string(),
                confidence: confidence.clamp(0.0, 0.5),
            })
        }
    }
}
