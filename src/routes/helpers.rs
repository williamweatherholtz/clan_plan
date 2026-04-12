use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{reunion::Reunion, user::User},
    state::AppState,
};

/// Load a reunion by ID or return 404.
pub async fn load_reunion(state: &AppState, id: Uuid) -> AppResult<Reunion> {
    Reunion::find_by_id(state.db(), id).await
}

/// Return Forbidden if the user is neither the RA for this reunion nor a sysadmin.
pub fn ensure_ra(user: &User, reunion: &Reunion) -> AppResult<()> {
    if user.is_ra_for(reunion.responsible_admin_id) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
