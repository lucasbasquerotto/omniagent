use sql_forge::sql_forge;
use sqlx::PgPool;

/// Update a kanban task's status by task_id.
pub async fn update_kanban_status(pool: &PgPool, task_id: &str, status: &str) -> anyhow::Result<()> {
    sql_forge!(
        "UPDATE kanban_tasks SET status = :status, updated_at = NOW() WHERE id = :id",
        ( :status = status, :id = task_id )
    )
    .execute(pool)
    .await?;
    Ok(())
}
