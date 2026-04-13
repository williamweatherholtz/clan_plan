use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    auth::session::CurrentUser,
    error::{AppError, AppResult},
    models::expense::{Expense, ExpenseSplit, NewExpense},
    state::AppState,
};

use super::helpers::{ensure_ra, load_reunion};

// ── Response type ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ExpenseWithSplits {
    #[serde(flatten)]
    pub expense: Expense,
    pub splits: Vec<ExpenseSplit>,
}

// ── GET /reunions/:id/expenses ────────────────────────────────────────────────

pub async fn list_expenses(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let expenses = Expense::list_for_reunion(state.db(), reunion_id).await?;
    let mut result = Vec::with_capacity(expenses.len());
    for expense in expenses {
        let splits = ExpenseSplit::list_for_expense(state.db(), expense.id).await?;
        result.push(ExpenseWithSplits { expense, splits });
    }

    Ok(Json(result))
}

// ── POST /reunions/:id/expenses ───────────────────────────────────────────────

pub async fn create_expense(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
    Json(body): Json<NewExpense>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;

    let expense = Expense::create(state.db(), reunion_id, user.id, body).await?;
    Ok((StatusCode::CREATED, Json(expense)))
}

// ── DELETE /reunions/:id/expenses/:exp_id ────────────────────────────────────
// RA only — expense records are financial, so only the RA can remove them.

pub async fn delete_expense(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((reunion_id, exp_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    ensure_ra(&user, &state, reunion_id).await?;

    let expense = Expense::find_by_id(state.db(), exp_id).await?;
    if expense.reunion_id != reunion_id {
        return Err(AppError::NotFound);
    }

    Expense::delete(state.db(), exp_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /reunions/:id/expenses/balances ───────────────────────────────────────

pub async fn get_balances(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let balances = Expense::balances_for_reunion(state.db(), reunion_id).await?;
    Ok(Json(balances))
}

// ── GET /reunions/:id/expenses/balances.csv ───────────────────────────────────

pub async fn get_balances_csv(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    load_reunion(&state, reunion_id).await?;
    let balances = Expense::balances_for_reunion(state.db(), reunion_id).await?;

    let mut csv = String::from("user_id,net_cents,net_dollars\n");
    for b in &balances {
        csv.push_str(&format!(
            "{},{},{:.2}\n",
            b.user_id,
            b.net_cents,
            b.net_cents as f64 / 100.0
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"balances.csv\""),
    );

    Ok((StatusCode::OK, headers, csv))
}

// ── POST /reunions/:id/expenses/confirm ──────────────────────────────────────
// Any member marks their own expense entries as complete for this reunion.

pub async fn confirm_expenses(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    load_reunion(&state, reunion_id).await?;
    sqlx::query(
        "INSERT INTO expense_confirmations (reunion_id, user_id)
         VALUES ($1, $2)
         ON CONFLICT (reunion_id, user_id) DO NOTHING",
    )
    .bind(reunion_id)
    .bind(user.id)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /reunions/:id/expenses/confirm ─────────────────────────────────────

pub async fn unconfirm_expenses(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(reunion_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    sqlx::query(
        "DELETE FROM expense_confirmations WHERE reunion_id = $1 AND user_id = $2",
    )
    .bind(reunion_id)
    .bind(user.id)
    .execute(state.db())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::models::expense::NewExpense;
    use chrono::NaiveDate;
    use uuid::Uuid;

    #[test]
    fn new_expense_deserializes() {
        let id = Uuid::new_v4();
        let json = format!(
            r#"{{"paid_by_user_id":"{id}","description":"Groceries","amount_cents":5000,
                "expense_date":"2026-07-12","split_among":["{id}"]}}"#
        );
        let req: NewExpense = serde_json::from_str(&json).unwrap();
        assert_eq!(req.amount_cents, 5000);
        assert_eq!(req.expense_date, Some(NaiveDate::from_ymd_opt(2026, 7, 12).unwrap()));
    }

    #[test]
    fn csv_row_format() {
        // Verify the float formatting used in get_balances_csv
        let net_cents: i64 = 1050;
        let formatted = format!("{:.2}", net_cents as f64 / 100.0);
        assert_eq!(formatted, "10.50");
    }

    #[test]
    fn negative_balance_csv_row() {
        let net_cents: i64 = -333;
        let formatted = format!("{:.2}", net_cents as f64 / 100.0);
        assert_eq!(formatted, "-3.33");
    }
}
