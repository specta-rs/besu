pub struct InsertQuery {}

impl InsertQuery {
    pub async fn value<V: InsertValue>(self, value: V) -> Result<(), ()> {
        todo!();
    }

    pub async fn values<V: InsertValue, I: IntoIterator<Item = V>>(
        self,
        values: I,
    ) -> Result<(), ()> {
        todo!();
    }
}

pub trait InsertValue {}

// Required for `&[]` because it yields `&V` not `V` as it can't be consumed.
impl<V: InsertValue> InsertValue for &V {}
