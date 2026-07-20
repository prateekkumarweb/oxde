#[derive(Debug, Clone, toasty::Model)]
pub struct User {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub username: String,
    pub password_hash: String,
    /// `"admin"` or `"member"`, see `accounts::AccountRole` in the `oxde`
    /// crate. Stored as `String` rather than a native enum column -
    /// simplest thing the ORM is known to support well.
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}
