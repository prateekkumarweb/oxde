use crate::{
    accounts::AccountRole,
    auth::CurrentUser,
    error::{AppError, AppResult},
    models::{App, PermissionLevel},
};

pub fn check_app_permission(
    user: &CurrentUser,
    app: &App,
    required: PermissionLevel,
) -> AppResult<()> {
    match user.role {
        AccountRole::Admin => Ok(()),
        AccountRole::Member if app.has_permission(&user.username, required) => Ok(()),
        AccountRole::Member => Err(AppError::Forbidden(format!(
            "you don't have {} access to this app",
            match required {
                PermissionLevel::Read => "read",
                PermissionLevel::Write => "write",
            }
        ))),
    }
}
