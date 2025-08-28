use crate::{Argument, Driver, Sql};

pub struct Db<D>(pub(crate) D);

impl<D: Driver> Db<D> {
    pub fn new(driver: D) -> Self {
        Db(driver)
    }

    // TODO: Results (row or stats)
    // TODO: Streaming
    pub async fn execute<'a, T: Argument<D>>(&self, sql: Sql<T>) -> Result<(), D::Error> {
        self.0.execute(sql.into_dialect()).await
    }
}

// TODO: Maybe?
// impl AsRef<MySqlPool> for crate::Db<MySql> {
//     fn as_ref(&self) -> &MySqlPool {
//         &self.0 .0
//     }
// }
