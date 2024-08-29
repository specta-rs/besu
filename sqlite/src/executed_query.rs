// TODO: This should probs come from the driver????
#[derive(Debug, Clone)]
pub struct ExecutedQuery {
    // `Option` should be thought of as an empty `Vec` and is an internal optimization.
    // pub rows: Option<Vec<Vec<SqlValue>>>,
    pub rows_affected: u64,
    pub last_insert_id: i64,
    // statement: String
    // time: u64,
}

impl ExecutedQuery {
    // pub fn rows(&self) -> Vec<Vec<SqlValue>> {
    //     self.rows.clone().unwrap_or_default()
    // }

    pub fn rows_affected(&self) -> u64 {
        self.rows_affected
    }

    pub fn last_insert_id(&self) -> i64 {
        self.last_insert_id
    }
}
