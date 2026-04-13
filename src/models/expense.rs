use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Expense {
    pub id: Uuid,
    pub reunion_id: Uuid,
    pub logged_by: Uuid,
    pub paid_by_user_id: Uuid,
    pub description: String,
    /// Stored in cents.
    pub amount_cents: i32,
    pub expense_date: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExpenseSplit {
    pub id: Uuid,
    pub expense_id: Uuid,
    pub user_id: Uuid,
    pub amount_cents: i32,
}

#[derive(Debug, Deserialize)]
pub struct NewExpense {
    pub paid_by_user_id: Uuid,
    pub description: String,
    /// In cents.
    pub amount_cents: i32,
    /// Defaults to today if omitted.
    pub expense_date: Option<NaiveDate>,
    /// Who is splitting this expense. If empty, defaults to all reunion members —
    /// but the caller should pass all member IDs; the empty check is a fallback guard.
    pub split_among: Vec<Uuid>,
}

/// Running balance per member: positive means they are owed money.
#[derive(Debug, Clone, Serialize)]
pub struct MemberBalance {
    pub user_id: Uuid,
    pub net_cents: i64,
}

// ── Pure business logic ────────────────────────────────────────────────────────

/// Distribute `total_cents` evenly across `members`.
/// Remainder cents (from integer division) are distributed to the first N members.
///
/// Invariant: `sum(result) == total_cents` (no cents lost or created).
pub fn calculate_even_split(total_cents: i32, members: &[Uuid]) -> Vec<(Uuid, i32)> {
    let n = members.len() as i32;
    if n == 0 {
        return vec![];
    }
    let base = total_cents / n;
    let extra = total_cents % n; // number of members who get one extra cent
    members
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let amount = base + if (i as i32) < extra { 1 } else { 0 };
            (id, amount)
        })
        .collect()
}

// ── DB queries ─────────────────────────────────────────────────────────────────

impl Expense {
    /// Create an expense and its splits in one transaction.
    pub async fn create(
        pool: &PgPool,
        reunion_id: Uuid,
        logged_by: Uuid,
        new: NewExpense,
    ) -> AppResult<Expense> {
        if new.amount_cents <= 0 {
            return Err(AppError::BadRequest("amount must be greater than zero".into()));
        }
        if new.split_among.is_empty() {
            return Err(AppError::BadRequest("split_among must not be empty — pass all member IDs".into()));
        }

        let expense_date = new.expense_date.unwrap_or_else(|| Local::now().date_naive());
        let splits = calculate_even_split(new.amount_cents, &new.split_among);

        let mut tx = pool.begin().await?;

        let expense = sqlx::query_as::<_, Expense>(
            r#"INSERT INTO expenses
               (reunion_id, logged_by, paid_by_user_id, description, amount_cents, expense_date)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(reunion_id)
        .bind(logged_by)
        .bind(new.paid_by_user_id)
        .bind(&new.description)
        .bind(new.amount_cents)
        .bind(expense_date)
        .fetch_one(&mut *tx)
        .await?;

        for (user_id, amount_cents) in splits {
            sqlx::query(
                r#"INSERT INTO expense_splits (expense_id, user_id, amount_cents)
                   VALUES ($1, $2, $3)"#,
            )
            .bind(expense.id)
            .bind(user_id)
            .bind(amount_cents)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(expense)
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Expense> {
        sqlx::query_as::<_, Expense>("SELECT * FROM expenses WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(AppError::NotFound)
    }

    pub async fn list_for_reunion(pool: &PgPool, reunion_id: Uuid) -> AppResult<Vec<Expense>> {
        Ok(sqlx::query_as::<_, Expense>(
            "SELECT * FROM expenses WHERE reunion_id = $1 ORDER BY expense_date, created_at",
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?)
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
        // Splits are cascade-deleted by the FK constraint
        sqlx::query("DELETE FROM expenses WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Compute each member's net balance across all expenses for a reunion.
    /// Positive net_cents = they are owed money; negative = they owe money.
    pub async fn balances_for_reunion(
        pool: &PgPool,
        reunion_id: Uuid,
    ) -> AppResult<Vec<MemberBalance>> {
        // Paid = money out of pocket; owed = their share of expenses
        let rows = sqlx::query_as::<_, (Uuid, i64)>(
            r#"SELECT
                u.id AS user_id,
                COALESCE(paid.total, 0) - COALESCE(owed.total, 0) AS net_cents
               FROM (
                   SELECT DISTINCT user_id FROM expense_splits es
                   JOIN expenses e ON e.id = es.expense_id
                   WHERE e.reunion_id = $1
                   UNION
                   SELECT DISTINCT paid_by_user_id FROM expenses WHERE reunion_id = $1
               ) u(id)
               LEFT JOIN (
                   SELECT paid_by_user_id, SUM(amount_cents) AS total
                   FROM expenses WHERE reunion_id = $1
                   GROUP BY paid_by_user_id
               ) paid ON paid.paid_by_user_id = u.id
               LEFT JOIN (
                   SELECT es.user_id, SUM(es.amount_cents) AS total
                   FROM expense_splits es
                   JOIN expenses e ON e.id = es.expense_id
                   WHERE e.reunion_id = $1
                   GROUP BY es.user_id
               ) owed ON owed.user_id = u.id
               ORDER BY net_cents DESC"#,
        )
        .bind(reunion_id)
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(user_id, net_cents)| MemberBalance { user_id, net_cents })
            .collect())
    }
}

impl ExpenseSplit {
    pub async fn list_for_expense(pool: &PgPool, expense_id: Uuid) -> AppResult<Vec<ExpenseSplit>> {
        Ok(sqlx::query_as::<_, ExpenseSplit>(
            "SELECT * FROM expense_splits WHERE expense_id = $1",
        )
        .bind(expense_id)
        .fetch_all(pool)
        .await?)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn uuids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn even_split_exact() {
        let members = uuids(4);
        let splits = calculate_even_split(100, &members);
        assert_eq!(splits.len(), 4);
        assert!(splits.iter().all(|(_, amt)| *amt == 25));
        let total: i32 = splits.iter().map(|(_, a)| a).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn even_split_with_remainder() {
        let members = uuids(3);
        let splits = calculate_even_split(10, &members);
        // 10 / 3 = 3 remainder 1: first member gets 4, rest get 3
        let amounts: Vec<i32> = splits.iter().map(|(_, a)| *a).collect();
        assert_eq!(amounts[0], 4);
        assert_eq!(amounts[1], 3);
        assert_eq!(amounts[2], 3);
        let total: i32 = amounts.iter().sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn split_sum_invariant() {
        // No cents should be lost for any combo of amount + member count
        for total in [1, 7, 99, 100, 101, 1000, 9999] {
            for n in 1usize..=10 {
                let members = uuids(n);
                let splits = calculate_even_split(total, &members);
                let sum: i32 = splits.iter().map(|(_, a)| *a).sum();
                assert_eq!(sum, total, "total={total}, n={n}");
            }
        }
    }

    #[test]
    fn empty_members_returns_empty() {
        assert!(calculate_even_split(100, &[]).is_empty());
    }

    #[test]
    fn single_member_gets_all() {
        let members = uuids(1);
        let splits = calculate_even_split(77, &members);
        assert_eq!(splits[0].1, 77);
    }
}
