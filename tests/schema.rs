use besu::{table, Database};

// table!(User, {
//     id: u64,
//     name: String,
//     age: u8
// });

// TODO: Automate this all!

#[allow(non_upper_case_globals)]
pub const User: () = ();

#[allow(non_snake_case)]
pub mod User {
    pub const id: () = ();
    pub const enabled: () = ();

    #[allow(non_camel_case_types)]
    pub struct insert {
        pub name: String,
        pub password: Option<String>,
    }

    // TODO: `&mut self` vs `self`?
    impl insert {
        pub fn edit(&mut self) -> insert_mut {
            insert_mut(self)
        }

        pub fn password(mut self, password: impl Into<String>) -> Self {
            self.password = Some(password.into());
            self
        }
    }

    // TODO: Sealed type
    pub struct insert_mut<'a>(&'a mut insert);

    impl<'a> insert_mut<'a> {
        pub fn password(&mut self, password: impl Into<String>) {
            self.0.password = Some(password.into());
        }
    }

    impl besu::queries::InsertValue for insert {}

    pub fn insert(name: impl Into<String>) -> insert {
        insert {
            name: name.into(),
            password: None,
        }
    }
}

#[allow(non_snake_case)]
pub mod Post {
    pub const id: () = ();
}

pub async fn init() -> Database {
    let db = Database::new(":memory:").await.unwrap();
    // TODO: Migrations
    db
}
