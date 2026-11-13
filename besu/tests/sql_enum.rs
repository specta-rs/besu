//! `#[derive(SqlEnum)]` names a column's variants once and implements everything a column
//! type needs: its [`SqlType`] definition, the label conversion in both directions, and the
//! bound value.
#![allow(missing_docs, clippy::unwrap_used, clippy::panic)]

use besu::prelude::*;
use besu::{EnumDef, SqlEnum, UnknownLabel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, besu::SqlEnum)]
#[besu(rename_all = "snake_case")]
pub enum Mood {
    Happy,
    #[besu(rename = "meh")]
    Neutral,
    Sad,
}

#[test]
fn the_derive_produces_the_type_and_its_labels() {
    assert_eq!(
        <Mood as SqlType>::DEF,
        TypeDef::Enum(EnumDef::new("mood", &["happy", "meh", "sad"])),
        "the type carries the enum's name and every label, in order"
    );
    assert_eq!(<Mood as SqlEnum>::LABELS, ["happy", "meh", "sad"]);
}

#[test]
fn a_variant_maps_to_its_label_and_back() {
    assert_eq!(Mood::Happy.label(), "happy");
    assert_eq!(Mood::Neutral.label(), "meh");
    assert_eq!(Mood::from_label("meh"), Ok(Mood::Neutral));
    assert_eq!(Mood::from_label("sad"), Ok(Mood::Sad));

    let unknown = Mood::from_label("angry").expect_err("an unknown label must not decode");
    assert_eq!(unknown, UnknownLabel::new("angry"));
    assert_eq!(unknown.label(), "angry");
}

#[test]
fn a_variant_becomes_the_label_text() {
    // The label is text; the `EnumLabel` variant tells the PostgreSQL driver to bind it
    // untyped, so the server can infer the enum from the column.
    assert_eq!(Value::from(Mood::Sad), Value::EnumLabel("sad".into()));
}

#[test]
fn the_enum_name_defaults_to_snake_case() {
    #[derive(Clone, Copy, besu::SqlEnum)]
    enum UserRole {
        Admin,
    }

    let TypeDef::Enum(def) = <UserRole as SqlType>::DEF else {
        panic!("the type must be an enum");
    };
    assert_eq!(def.name, "user_role");
    // Without `rename_all` the label is the variant's name as written.
    assert_eq!(<UserRole as SqlEnum>::LABELS, ["Admin"]);
}

#[test]
fn every_rename_rule_is_spelled_the_way_serde_spells_it() {
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "camelCase")]
    enum Camel {
        VeryHappy,
    }
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "PascalCase")]
    enum Pascal {
        VeryHappy,
    }
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "SCREAMING_SNAKE_CASE")]
    enum Screaming {
        VeryHappy,
    }
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "lowercase")]
    enum Lower {
        Happy,
    }
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "UPPERCASE")]
    enum Upper {
        Happy,
    }
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(rename_all = "kebab-case")]
    enum Kebab {
        VeryHappy,
    }

    assert_eq!(<Camel as SqlEnum>::LABELS, ["veryHappy"]);
    assert_eq!(<Pascal as SqlEnum>::LABELS, ["VeryHappy"]);
    assert_eq!(<Screaming as SqlEnum>::LABELS, ["VERY_HAPPY"]);
    assert_eq!(<Lower as SqlEnum>::LABELS, ["happy"]);
    assert_eq!(<Upper as SqlEnum>::LABELS, ["HAPPY"]);
    assert_eq!(<Kebab as SqlEnum>::LABELS, ["very-happy"]);
}

#[test]
fn a_renamed_enum_uses_the_name_it_was_given() {
    #[derive(Clone, Copy, besu::SqlEnum)]
    #[besu(name = "public.mood")]
    enum Mood {
        Happy,
    }

    let TypeDef::Enum(def) = <Mood as SqlType>::DEF else {
        panic!("the type must be an enum");
    };
    assert_eq!(def.name, "public.mood");
}
