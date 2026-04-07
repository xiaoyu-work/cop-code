use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse};

use super::openai_compat::{OpenAiCompatClient, OpenAiCompatConfig};
use super::{Provider, ProviderFuture};

const COPILOT_PROVIDER_NAME: &str = "GitHub Copilot";
const COPILOT_GITHUB_TOKEN_ENV_VARS: &[&str] = &["COPILOT_GITHUB_TOKEN", "GITHUB_TOKEN"];
const COPILOT_TOKEN_REFRESH_SAFETY_WINDOW_MS: u64 = 5 * 60 * 1000;
const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;

pub const GITHUB_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
pub const DEFAULT_BASE_URL: &str = "https://api.individual.githubcopilot.com";
pub const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const OAUTH_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const TOKEN_EXCHANGE_URL: &str = "https://api.github.com/copilot_internal/v2/token";
pub const EDITOR_VERSION: &str = "vscode/1.96.2";
pub const INTEGRATION_ID: &str = "vscode-chat";
pub const DEVICE_CODE_SCOPE: &str = "read:user";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone)]
pub struct CopilotClient {
    http: Client,
    github_token: String,
    base_url_override: Option<String>,
    cached_token: std::sync::Arc<std::sync::Mutex<Option<CachedCopilotToken>>>,
}

#[derive(Debug, Clone)]
struct CachedCopilotToken {
    access_token: String,
    expires_at_millis: Option<u64>,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct RawDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: Option<String>,
    verification_url: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitHubAccessTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
}

impl CopilotClient {
    #[must_use]
    pub fn new(github_token: impl Into<String>) -> Self {
        Self {
            http: copilot_http_client(),
            github_token: github_token.into(),
            base_url_override: None,
            cached_token: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn with_base_url_override(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    pub fn from_env_or_saved() -> Result<Self, ApiError> {
        let github_token = read_first_non_empty_env(COPILOT_GITHUB_TOKEN_ENV_VARS)?
            .or_else(load_saved_github_token);
        let Some(github_token) = github_token else {
            return Err(ApiError::missing_credentials(
                COPILOT_PROVIDER_NAME,
                COPILOT_GITHUB_TOKEN_ENV_VARS,
            ));
        };

        let mut client = Self::new(github_token);
        let base_url_override = read_base_url();
        if base_url_override != DEFAULT_BASE_URL {
            client = client.with_base_url_override(base_url_override);
        }
        Ok(client)
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let delegate = self.delegate_client().await?;
        delegate.send_message(request).await
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<super::openai_compat::MessageStream, ApiError> {
        let delegate = self.delegate_client().await?;
        delegate.stream_message(request).await
    }

    async fn delegate_client(&self) -> Result<OpenAiCompatClient, ApiError> {
        let resolved = self.resolve_copilot_token().await?;
        Ok(OpenAiCompatClient::new(
            resolved.access_token,
            OpenAiCompatConfig {
                provider_name: COPILOT_PROVIDER_NAME,
                api_key_env: "COPILOT_API_KEY",
                base_url_env: "COPILOT_BASE_URL",
                default_base_url: DEFAULT_BASE_URL,
            },
        )
        .with_base_url(resolved.base_url)
        .with_extra_headers(copilot_headers()))
    }

    async fn resolve_copilot_token(&self) -> Result<CachedCopilotToken, ApiError> {
        if let Some(cached) = self
            .cached_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .filter(CachedCopilotToken::is_usable)
        {
            return Ok(cached);
        }

        let refreshed = self.exchange_copilot_token().await?;
        *self
            .cached_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(refreshed.clone());
        Ok(refreshed)
    }

    async fn exchange_copilot_token(&self) -> Result<CachedCopilotToken, ApiError> {
        let response = self
            .http
            .get(TOKEN_EXCHANGE_URL)
            .header("accept", "application/json")
            .bearer_auth(&self.github_token)
            .send()
            .await
            .map_err(ApiError::from)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::Auth(format!(
                "GitHub Copilot token exchange failed ({status}): {body}"
            )));
        }

        let payload = response
            .json::<CopilotTokenResponse>()
            .await
            .map_err(ApiError::from)?;
        let access_token = payload
            .token
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApiError::Auth("GitHub Copilot token exchange returned no token".to_string())
            })?;
        let base_url = self
            .base_url_override
            .clone()
            .unwrap_or_else(|| derive_base_url_from_token(&access_token));
        Ok(CachedCopilotToken {
            access_token,
            expires_at_millis: payload.expires_at.map(|value| value.saturating_mul(1000)),
            base_url,
        })
    }
}

impl CachedCopilotToken {
    fn is_usable(&self) -> bool {
        self.expires_at_millis.is_none_or(|expires_at| {
            expires_at.saturating_sub(COPILOT_TOKEN_REFRESH_SAFETY_WINDOW_MS) > now_millis()
        })
    }
}

impl Provider for CopilotClient {
    type Stream = super::openai_compat::MessageStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

pub async fn request_device_code() -> Result<DeviceCodeResponse, ApiError> {
    let response = copilot_http_client()
        .post(DEVICE_CODE_URL)
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .form(&[
            ("client_id", GITHUB_CLIENT_ID),
            ("scope", DEVICE_CODE_SCOPE),
        ])
        .send()
        .await
        .map_err(ApiError::from)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::Auth(format!(
            "GitHub device code request failed ({status}): {body}"
        )));
    }

    let payload = response
        .json::<RawDeviceCodeResponse>()
        .await
        .map_err(ApiError::from)?;
    Ok(DeviceCodeResponse {
        device_code: payload.device_code,
        user_code: payload.user_code,
        verification_uri: payload
            .verification_uri
            .or(payload.verification_url)
            .unwrap_or_else(|| "https://github.com/login/device".to_string()),
        expires_in: payload.expires_in,
        interval: payload
            .interval
            .unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL_SECS),
    })
}

pub async fn poll_device_code_token(
    device_code: &str,
    interval_secs: u64,
) -> Result<runtime::OAuthTokenSet, ApiError> {
    let client = copilot_http_client();
    let poll_interval = interval_secs.max(DEFAULT_DEVICE_POLL_INTERVAL_SECS);
    loop {
        tokio::time::sleep(Duration::from_secs(poll_interval)).await;
        let response = client
            .post(OAUTH_TOKEN_URL)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", GITHUB_CLIENT_ID),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(ApiError::from)?;
        let payload = response
            .json::<GitHubAccessTokenResponse>()
            .await
            .map_err(ApiError::from)?;

        if let Some(access_token) = payload.access_token.filter(|value| !value.is_empty()) {
            return Ok(runtime::OAuthTokenSet {
                access_token,
                refresh_token: payload.refresh_token,
                expires_at: payload
                    .expires_in
                    .map(|seconds| now_unix_seconds() + seconds),
                scopes: vec![DEVICE_CODE_SCOPE.to_string()],
            });
        }

        match payload.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Some("expired_token") => {
                return Err(ApiError::Auth("GitHub device code expired".to_string()))
            }
            Some("access_denied") => {
                return Err(ApiError::Auth(
                    "GitHub device authorization denied".to_string(),
                ))
            }
            Some(error) => {
                return Err(ApiError::Auth(format!(
                    "GitHub device code token polling failed: {}",
                    payload
                        .error_description
                        .unwrap_or_else(|| error.to_string())
                )))
            }
            None => {
                return Err(ApiError::Auth(
                    "GitHub device code token polling returned no token".to_string(),
                ))
            }
        }
    }
}

#[must_use]
pub fn derive_base_url_from_token(token: &str) -> String {
    let Some(proxy_host) = token
        .split(';')
        .map(str::trim)
        .find_map(|segment| segment.strip_prefix("proxy-ep="))
    else {
        return DEFAULT_BASE_URL.to_string();
    };
    let host = proxy_host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let api_host = host.replace("proxy.", "api.");
    format!("https://{api_host}")
}

pub fn has_github_token_from_env_or_saved() -> Result<bool, ApiError> {
    Ok(
        read_first_non_empty_env(COPILOT_GITHUB_TOKEN_ENV_VARS)?.is_some()
            || load_saved_github_token().is_some(),
    )
}

#[must_use]
pub fn read_base_url() -> String {
    std::env::var("COPILOT_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

fn load_saved_github_token() -> Option<String> {
    match runtime::load_provider_oauth_credentials("copilot") {
        Ok(Some(token_set)) => {
            if token_set
                .expires_at
                .is_some_and(|expires_at| expires_at <= now_unix_seconds())
            {
                None
            } else {
                Some(token_set.access_token)
            }
        }
        Ok(None) | Err(_) => None,
    }
}

fn read_first_non_empty_env(keys: &[&str]) -> Result<Option<String>, ApiError> {
    for key in keys {
        match std::env::var(key) {
            Ok(value) if !value.is_empty() => return Ok(Some(value)),
            Ok(_) | Err(std::env::VarError::NotPresent) => {}
            Err(error) => return Err(ApiError::from(error)),
        }
    }
    Ok(None)
}

fn copilot_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Editor-Version".to_string(), EDITOR_VERSION.to_string()),
        (
            "Copilot-Integration-Id".to_string(),
            INTEGRATION_ID.to_string(),
        ),
    ])
}

fn copilot_http_client() -> Client {
    Client::builder()
        .user_agent(format!("{EDITOR_VERSION} {INTEGRATION_ID}"))
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::derive_base_url_from_token;

    #[test]
    fn derives_api_base_url_from_proxy_endpoint_token_field() {
        assert_eq!(
            derive_base_url_from_token("abc; proxy-ep=proxy.enterprise.githubcopilot.com; xyz"),
            "https://api.enterprise.githubcopilot.com"
        );
    }

    #[test]
    fn falls_back_to_default_base_url_when_proxy_endpoint_is_missing() {
        assert_eq!(
            derive_base_url_from_token("abc;something=else"),
            super::DEFAULT_BASE_URL
        );
    }
}
