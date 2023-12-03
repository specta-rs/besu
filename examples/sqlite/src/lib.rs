use besu::table;

table!(User, users, {
    id: |t| t.serial("id"),
    name: |t| t.text("name"),
    age: |t| t.int("age"),
    bio: |t| t.text("bio").default("Cool"),
    phone: |t| t.text("phone").optional(),
    // TODO: Many to many `posts: Many<Post>`,
});

table!(Post, posts, {
    id: |t| t.serial("id"),
    title: |t| t.text("title"),
    // TODO: Rest of it
});
