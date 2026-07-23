// @generated automatically by Diesel CLI.

diesel::table! {
    #[allow(non_snake_case)]
    LocalRepo (name) {
        name -> Text,
        version -> Text,
        url -> Text,
        description -> Text,
        filename -> Text,
        hash -> Text,
        entry -> Text,
        dependencies -> Nullable<Text>,
    }
}
