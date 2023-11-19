// use besu::WhereCondition;
// use schema::{posts, users};

mod schema;

// Typing where bounds
// pub type UserIdEqualsConstant = WhereCondition<users::id, i32>;
// pub type UserIdEqualsPostId = WhereCondition<users::id, posts::owner_id>;

// TODO: Build a function with a group of `WHERE` clauses and apply them to a query, insert, update, etc.
// TODO: Allow the abstraction to easily support *any* model

// fn user<M>(id: u32) -> Query<M> {
//     // db.query(users)._where(());
// }
