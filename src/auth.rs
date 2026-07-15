use axum::{
    body::Body,
    http::{HeaderValue, Request, Response, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tower_http::validate_request::ValidateRequest;

#[derive(Clone)]
pub struct BasicAuth {
    username: String,
    password: String,
}

impl BasicAuth {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    fn credentials_valid(&self, header_value: &HeaderValue) -> bool {
        let Ok(value) = header_value.to_str() else {
            return false;
        };
        let Some(encoded) = value.strip_prefix("Basic ") else {
            return false;
        };
        let Ok(decoded) = BASE64.decode(encoded) else {
            return false;
        };
        let Ok(decoded) = String::from_utf8(decoded) else {
            return false;
        };
        let Some((user, pass)) = decoded.split_once(':') else {
            return false;
        };
        user == self.username && pass == self.password
    }
}

impl<B> ValidateRequest<B> for BasicAuth {
    type ResponseBody = Body;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<Self::ResponseBody>> {
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .is_some_and(|value| self.credentials_valid(value));

        if authorized {
            return Ok(());
        }

        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::UNAUTHORIZED;
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"oxde\""),
        );
        Err(response)
    }
}
