//! Admin API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use super::oauth::OauthSessions;
use super::service::AdminService;
use super::types::AdminErrorResponse;
use crate::common::auth;
use crate::http_client::ProxyConfig;
use crate::model::config::TlsBackend;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥
    pub admin_api_key: String,
    /// Admin 服务
    pub service: Arc<AdminService>,
    /// OAuth 登录会话注册表
    pub oauth_sessions: Arc<OauthSessions>,
    /// 全局代理配置（OAuth 流程默认使用）
    pub proxy: Option<ProxyConfig>,
    /// TLS 后端
    pub tls_backend: TlsBackend,
}

impl AdminState {
    pub fn new(
        admin_api_key: impl Into<String>,
        service: AdminService,
        proxy: Option<ProxyConfig>,
        tls_backend: TlsBackend,
    ) -> Self {
        Self {
            admin_api_key: admin_api_key.into(),
            service: Arc::new(service),
            oauth_sessions: Arc::new(OauthSessions::new()),
            proxy,
            tls_backend,
        }
    }
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let api_key = auth::extract_api_key(&request);

    match api_key {
        Some(key) if auth::constant_time_eq(&key, &state.admin_api_key) => next.run(request).await,
        _ => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
