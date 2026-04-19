//! Builder ID 设备码登录流程
//!
//! 实现 AWS OIDC 设备码（device code）授权，用于引导用户在浏览器中完成
//! Builder ID 登录，成功后自动把新的 refreshToken 写入 credentials.json。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::http_client::{ProxyConfig, build_client};

use super::middleware::AdminState;
use super::types::{AddCredentialRequest, AdminErrorResponse, SuccessResponse};

const BUILDER_ID_START_URL: &str = "https://view.awsapps.com/start";
const OAUTH_CLIENT_NAME: &str = "kiro-rs";
const DEFAULT_REGION: &str = "us-east-1";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const SLOW_DOWN_BACKOFF_SECS: u64 = 5;
const OIDC_SCOPES: &[&str] = &[
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
    "codewhisperer:transformations",
    "codewhisperer:taskassist",
];

// ============ 会话状态 ============

#[derive(Debug, Clone)]
pub struct OauthSession {
    pub client_id: String,
    pub client_secret: String,
    pub device_code: String,
    pub region: String,
    pub expires_at: Instant,
    pub interval: u64,
    pub meta: OauthMeta,
}

#[derive(Debug, Clone, Default)]
pub struct OauthMeta {
    pub priority: u32,
    pub machine_id: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub endpoint: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Default)]
pub struct OauthSessions {
    inner: Mutex<HashMap<u64, OauthSession>>,
}

impl OauthSessions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, id: u64, session: OauthSession) {
        self.inner.lock().insert(id, session);
    }

    pub fn get(&self, id: u64) -> Option<OauthSession> {
        self.inner.lock().get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> Option<OauthSession> {
        self.inner.lock().remove(&id)
    }

    pub fn increase_interval(&self, id: u64, extra_secs: u64) {
        if let Some(session) = self.inner.lock().get_mut(&id) {
            session.interval = session.interval.saturating_add(extra_secs);
        }
    }

    pub fn gc_expired(&self) {
        let now = Instant::now();
        self.inner.lock().retain(|_, s| s.expires_at > now);
    }
}

// ============ 请求 / 响应 ============

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartBuilderIdRequest {
    pub region: Option<String>,
    #[serde(default)]
    pub priority: u32,
    pub machine_id: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub endpoint: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartBuilderIdResponse {
    pub login_id: u64,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum PollBuilderIdResponse {
    #[serde(rename = "pending")]
    Pending { interval: u64 },
    #[serde(rename = "slow_down")]
    SlowDown { interval: u64 },
    #[serde(rename = "success")]
    Success {
        credential_id: u64,
        email: Option<String>,
        message: String,
    },
    #[serde(rename = "expired")]
    Expired { message: String },
    #[serde(rename = "denied")]
    Denied { message: String },
    #[serde(rename = "error")]
    Error { message: String },
}

// ============ AWS OIDC DTO ============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientRequest<'a> {
    client_name: &'a str,
    client_type: &'a str,
    scopes: &'a [&'a str],
    grant_types: &'a [&'a str],
    issuer_url: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientResponse {
    client_id: String,
    client_secret: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    start_url: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    grant_type: &'a str,
    device_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct TokenSuccess {
    #[serde(rename = "accessToken")]
    #[allow(dead_code)]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: Option<String>,
    #[serde(rename = "error_description", alias = "errorDescription")]
    error_description: Option<String>,
}

// ============ 处理器 ============

/// POST /api/admin/oauth/builder-id/start
pub async fn start_builder_id(
    State(state): State<AdminState>,
    Json(req): Json<StartBuilderIdRequest>,
) -> impl IntoResponse {
    // 清理过期会话
    state.oauth_sessions.gc_expired();

    let region = req
        .region
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string());

    let proxy = match parse_proxy(&req.proxy_url, &req.proxy_username, &req.proxy_password) {
        Ok(p) => p.or_else(|| state.proxy.clone()),
        Err(e) => return bad_request(format!("代理配置无效: {}", e)).into_response(),
    };

    let client = match build_client(proxy.as_ref(), 30, state.tls_backend) {
        Ok(c) => c,
        Err(e) => return internal_error(format!("构建 HTTP 客户端失败: {}", e)).into_response(),
    };

    // Step 1: 注册 OIDC 客户端
    let register_url = format!("https://oidc.{}.amazonaws.com/client/register", region);
    let register_body = RegisterClientRequest {
        client_name: OAUTH_CLIENT_NAME,
        client_type: "public",
        scopes: OIDC_SCOPES,
        grant_types: &[DEVICE_CODE_GRANT_TYPE, "refresh_token"],
        issuer_url: BUILDER_ID_START_URL,
    };
    let register: RegisterClientResponse =
        match post_json(&client, &register_url, &register_body).await {
            Ok(r) => r,
            Err(e) => return bad_gateway(format!("注册 OIDC 客户端失败: {}", e)).into_response(),
        };

    // Step 2: 启动设备授权
    let auth_url = format!("https://oidc.{}.amazonaws.com/device_authorization", region);
    let auth_body = DeviceAuthRequest {
        client_id: &register.client_id,
        client_secret: &register.client_secret,
        start_url: BUILDER_ID_START_URL,
    };
    let auth: DeviceAuthResponse = match post_json(&client, &auth_url, &auth_body).await {
        Ok(r) => r,
        Err(e) => return bad_gateway(format!("启动设备授权失败: {}", e)).into_response(),
    };

    // Step 3: 生成 loginId 并存储会话
    // 限制在 JS Number.MAX_SAFE_INTEGER (2^53 - 1) 以内，避免前端精度丢失
    const JS_SAFE_MAX: u64 = (1u64 << 53) - 1;
    let login_id = loop {
        let id = fastrand::u64(1..JS_SAFE_MAX);
        if state.oauth_sessions.get(id).is_none() {
            break id;
        }
    };

    let session = OauthSession {
        client_id: register.client_id,
        client_secret: register.client_secret,
        device_code: auth.device_code,
        region,
        expires_at: Instant::now() + Duration::from_secs(auth.expires_in),
        interval: auth.interval.max(1),
        meta: OauthMeta {
            priority: req.priority,
            machine_id: req.machine_id,
            auth_region: req.auth_region,
            api_region: req.api_region,
            proxy_url: req.proxy_url,
            proxy_username: req.proxy_username,
            proxy_password: req.proxy_password,
            endpoint: req.endpoint,
            email: req.email,
        },
    };
    state.oauth_sessions.insert(login_id, session);

    Json(StartBuilderIdResponse {
        login_id,
        user_code: auth.user_code,
        verification_uri: auth.verification_uri,
        verification_uri_complete: auth.verification_uri_complete,
        expires_in: auth.expires_in,
        interval: auth.interval,
    })
    .into_response()
}

/// POST /api/admin/oauth/builder-id/poll/:login_id
pub async fn poll_builder_id(
    State(state): State<AdminState>,
    Path(login_id): Path<u64>,
) -> impl IntoResponse {
    let session = match state.oauth_sessions.get(login_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(AdminErrorResponse::not_found("登录会话不存在或已过期")),
            )
                .into_response();
        }
    };

    if Instant::now() > session.expires_at {
        state.oauth_sessions.remove(login_id);
        return Json(PollBuilderIdResponse::Expired {
            message: "设备码已过期，请重新发起登录".to_string(),
        })
        .into_response();
    }

    let proxy = match parse_proxy(
        &session.meta.proxy_url,
        &session.meta.proxy_username,
        &session.meta.proxy_password,
    ) {
        Ok(p) => p.or_else(|| state.proxy.clone()),
        Err(e) => return bad_request(format!("代理配置无效: {}", e)).into_response(),
    };

    let client = match build_client(proxy.as_ref(), 30, state.tls_backend) {
        Ok(c) => c,
        Err(e) => return internal_error(format!("构建 HTTP 客户端失败: {}", e)).into_response(),
    };

    let token_url = format!("https://oidc.{}.amazonaws.com/token", session.region);
    let token_body = TokenRequest {
        client_id: &session.client_id,
        client_secret: &session.client_secret,
        grant_type: DEVICE_CODE_GRANT_TYPE,
        device_code: &session.device_code,
    };

    let response = match client.post(&token_url).json(&token_body).send().await {
        Ok(r) => r,
        Err(e) => return bad_gateway(format!("轮询 Token 失败: {}", e)).into_response(),
    };

    let status = response.status();
    let body_text = response.text().await.unwrap_or_default();

    if status.is_success() {
        let data: TokenSuccess = match serde_json::from_str(&body_text) {
            Ok(d) => d,
            Err(e) => {
                return bad_gateway(format!("解析 Token 响应失败: {}", e)).into_response();
            }
        };
        let Some(refresh_token) = data.refresh_token else {
            return bad_gateway("Token 响应缺少 refreshToken").into_response();
        };

        // 登录成功：取走会话并构造 AddCredentialRequest
        let Some(session) = state.oauth_sessions.remove(login_id) else {
            return internal_error("会话已被并发消费").into_response();
        };

        let add_req = AddCredentialRequest {
            refresh_token: Some(refresh_token),
            auth_method: "idc".to_string(),
            client_id: Some(session.client_id),
            client_secret: Some(session.client_secret),
            priority: session.meta.priority,
            region: Some(session.region),
            auth_region: session.meta.auth_region,
            api_region: session.meta.api_region,
            machine_id: session.meta.machine_id,
            email: session.meta.email.clone(),
            proxy_url: session.meta.proxy_url,
            proxy_username: session.meta.proxy_username,
            proxy_password: session.meta.proxy_password,
            kiro_api_key: None,
            endpoint: session.meta.endpoint,
        };

        match state.service.add_credential(add_req).await {
            Ok(resp) => Json(PollBuilderIdResponse::Success {
                credential_id: resp.credential_id,
                email: resp.email,
                message: resp.message,
            })
            .into_response(),
            Err(e) => {
                let status = e.status_code();
                (status, Json(e.into_response())).into_response()
            }
        }
    } else {
        // 400 等错误，解析 error 字段
        let err: TokenError = serde_json::from_str(&body_text).unwrap_or(TokenError {
            error: None,
            error_description: None,
        });
        let error_code = err.error.as_deref().unwrap_or("");
        match error_code {
            "authorization_pending" => Json(PollBuilderIdResponse::Pending {
                interval: session.interval,
            })
            .into_response(),
            "slow_down" => {
                state
                    .oauth_sessions
                    .increase_interval(login_id, SLOW_DOWN_BACKOFF_SECS);
                let new_interval = state
                    .oauth_sessions
                    .get(login_id)
                    .map(|s| s.interval)
                    .unwrap_or(session.interval + SLOW_DOWN_BACKOFF_SECS);
                Json(PollBuilderIdResponse::SlowDown {
                    interval: new_interval,
                })
                .into_response()
            }
            "expired_token" => {
                state.oauth_sessions.remove(login_id);
                Json(PollBuilderIdResponse::Expired {
                    message: "设备码已过期，请重新发起登录".to_string(),
                })
                .into_response()
            }
            "access_denied" => {
                state.oauth_sessions.remove(login_id);
                Json(PollBuilderIdResponse::Denied {
                    message: "用户拒绝了授权".to_string(),
                })
                .into_response()
            }
            _ => {
                let detail = err
                    .error_description
                    .or(err.error)
                    .unwrap_or_else(|| format!("HTTP {} {}", status, body_text));
                Json(PollBuilderIdResponse::Error {
                    message: format!("登录失败: {}", detail),
                })
                .into_response()
            }
        }
    }
}

/// POST /api/admin/oauth/builder-id/cancel/:login_id
pub async fn cancel_builder_id(
    State(state): State<AdminState>,
    Path(login_id): Path<u64>,
) -> impl IntoResponse {
    let existed = state.oauth_sessions.remove(login_id).is_some();
    if existed {
        Json(SuccessResponse::new(format!("登录会话 #{} 已取消", login_id))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(AdminErrorResponse::not_found("登录会话不存在或已结束")),
        )
            .into_response()
    }
}

// ============ 辅助 ============

async fn post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    body: &T,
) -> anyhow::Result<R> {
    let resp = client.post(url).json(body).send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("HTTP {}: {}", status, text);
    }
    Ok(serde_json::from_str(&text)?)
}

fn parse_proxy(
    url: &Option<String>,
    username: &Option<String>,
    password: &Option<String>,
) -> anyhow::Result<Option<ProxyConfig>> {
    match url.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some("direct") => Ok(None),
        Some(u) => {
            let mut cfg = ProxyConfig::new(u);
            if let (Some(user), Some(pass)) = (username.as_deref(), password.as_deref()) {
                if !user.is_empty() {
                    cfg = cfg.with_auth(user, pass);
                }
            }
            Ok(Some(cfg))
        }
    }
}

fn bad_gateway(msg: impl Into<String>) -> (StatusCode, Json<AdminErrorResponse>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(AdminErrorResponse::api_error(msg)),
    )
}

fn internal_error(msg: impl Into<String>) -> (StatusCode, Json<AdminErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(AdminErrorResponse::internal_error(msg)),
    )
}

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<AdminErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(AdminErrorResponse::invalid_request(msg)),
    )
}
