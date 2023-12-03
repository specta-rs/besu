# Design

This document is designed to outline some of the design decisions made in this library.

## Schema definitions

Problems with a Rust-based schema:

It's really difficult to maintain an expressive syntax while not killing the LSP.

You could use attribute macros but express default values and relations in attributes is annoying.

You could use a DSL but then you get zero type hints.

What we do now also sucks because abstractions are not possible as the macro "reinterpret"'s the Rust code to derive it's own meaning but it's only aware of the tokens within the macros invoke scope.

It would be really nice if Rust let us infer the type of an expression such that it could be used in a struct but sadly that's not possible.

Another problem is also how to deal with database specific types. This one still needs investigation.
TODO

## Database capabilities

TODO: How do we handle them?

## `!Into<T> where T: Number`

Rust struggles to cast the number type automagically which hurts DX majorly hence the difference.

Eg. https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=9b591f627aea0931a536cb378ebde48d

## Inserts

SQLite lacks the `DEFAULT` keyword, this is a major problem. Each Besu query should generate a single, fixed SQL statement. This is important for prepared queries.

This however it tough for non-optional fields with default values. They effectively act to the user as optional but when it comes time to execute the SQL if the user didn't set a hardcoded value what do we set the value as????
TODO: Work this out

Something like Drizzle doesn't have this problem as they have no hardcoded `struct insert`. Each insert operation defines a local object of the fields it wants to provide. In Rust this is harder due to the horrible DX of existing macros.