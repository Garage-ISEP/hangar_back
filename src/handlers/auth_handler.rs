use axum::
{
    extract::{Query, State}, 
    response::{IntoResponse, Json}
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;

use crate::{error::AppError, state::AppState};
use crate::services::jwt::Claims;

#[derive(Debug, Deserialize)]
pub struct AuthCallbackQuery 
{
    ticket: String,
}

pub async fn auth_callback_handler(State(state): State<AppState>, 
                                   Query(query): Query<AuthCallbackQuery>, 
                                   jar: CookieJar) -> Result<impl IntoResponse, AppError>
{
    let service = format!("{}/auth/callback", state.config.public_address);

    let url = format!("{}?service={}&ticket={}", state.config.cas_validation_url, service, &query.ticket);
    tracing::debug!("Validating CAS ticket at URL: {}", url);
    let user = crate::services::auth_service::validate_ticket(&url, &state.http_client).await?;

    let is_admin = state.config.admin_logins.contains(&user.login);

    let token = crate::services::jwt::generate_jwt(
        &state.config.jwt_secret,
        state.config.jwt_expiration_seconds,
        &user.login,
        &user.name,
        &user.email,
        is_admin,
    )?;

    let cookie = Cookie::build(("auth_token", token))
        .path("/") // Le cookie est valide pour tout le site
        .secure(true) // Envoyé seulement sur HTTPS
        .http_only(true) // Inaccessible depuis JavaScript
        .same_site(SameSite::Lax) // Protection CSRF de base
        .build();
    
    Ok((
        jar.add(cookie),
        Json
        (
            json!
            (
                {
                    "message": "Authentication successful",
                    "user": 
                    {
                        "login": user.login,
                        "name": user.name,
                        "email": user.email,
                        "is_admin": is_admin
                    }
                }
            )
        ),
    ))

}

pub async fn get_current_user_handler(claims: Claims) -> impl IntoResponse 
{
    Json
    (
        json!
        (
            {
                "user": 
                {
                    "login": claims.sub,
                    "name": claims.name,
                    "email": claims.email,
                    "is_admin": claims.is_admin
                    
                }
            }
        )
    )
}


pub async fn logout_handler(jar: CookieJar) -> Result<impl IntoResponse, AppError> 
{
    let cookie = Cookie::build(("auth_token", ""))
        .path("/")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Lax)
        .expires(OffsetDateTime::UNIX_EPOCH) // Expire dans le passé
        .build();

    Ok((jar.add(cookie), axum::http::StatusCode::OK))
}