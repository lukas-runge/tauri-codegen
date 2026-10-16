//! Every `#[serde(rename_all = "...")]` variant applied to a struct, plus
//! `#[serde(rename)]` on individual fields.

use crate::helpers::{run_generate_ok, Project};

fn types_for(source: &str) -> String {
    let project = Project::with_source(source);
    run_generate_ok(&project);
    std::fs::read_to_string(&project.types_out).unwrap()
}

#[test]
fn no_attrs_preserves_original_field_names() {
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        pub struct Thing { pub user_id: i32, pub first_name: String }
        #[tauri::command]
        fn x() -> Result<Thing, String> { todo!() }
        "#,
    );
    // Plain snake_case must stay snake_case — it matches what serde emits.
    assert!(types.contains("user_id: number"), "{types}");
    assert!(types.contains("first_name: string"), "{types}");
    assert!(!types.contains("userId:"), "unexpected camelCase:\n{types}");
}

#[test]
fn camel_case() {
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Thing { pub user_id: i32, pub first_name: String }
        #[tauri::command]
        fn x() -> Result<Thing, String> { todo!() }
        "#,
    );
    assert!(types.contains("userId: number"), "{types}");
    assert!(types.contains("firstName: string"), "{types}");
}

#[test]
fn pascal_case() {
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "PascalCase")]
        pub struct Thing { pub user_id: i32, pub first_name: String }
        #[tauri::command]
        fn x() -> Result<Thing, String> { todo!() }
        "#,
    );
    assert!(types.contains("UserId: number"), "{types}");
    assert!(types.contains("FirstName: string"), "{types}");
}

#[test]
fn snake_case_transform_on_camel_input() {
    // Starting from a camelCase field, snake_case should rewrite it.
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub struct Thing { pub userId: i32 }
        #[tauri::command]
        fn x() -> Result<Thing, String> { todo!() }
        "#,
    );
    assert!(types.contains("user_id: number"), "{types}");
}

#[test]
fn screaming_snake_and_kebab() {
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub struct Loud { pub user_id: i32 }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub struct Kebab { pub user_id: i32 }

        #[tauri::command]
        fn x() -> Result<(Loud, Kebab), String> { todo!() }
        "#,
    );
    assert!(
        types.contains("USER_ID: number"),
        "SCREAMING_SNAKE:\n{types}"
    );
    // `user-id` is not an identifier, so the key must be quoted.
    assert!(types.contains(r#""user-id": number"#), "kebab:\n{types}");
}

#[test]
fn field_rename_overrides_container_rename_all() {
    let types = types_for(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Config {
            #[serde(rename = "API_KEY")]
            pub api_key: String,
            pub other_field: String,
        }
        #[tauri::command]
        fn x() -> Result<Config, String> { todo!() }
        "#,
    );
    assert!(
        types.contains("API_KEY: string"),
        "explicit rename:\n{types}"
    );
    assert!(
        types.contains("otherField: string"),
        "inherits camelCase:\n{types}"
    );
    assert!(
        !types.contains("apiKey:"),
        "rename must not fall through:\n{types}"
    );
}

#[test]
fn snake_case_handles_acronyms_correctly() {
    // Regression: `HTTPServer` must become `http_server`, not
    // `h_t_t_p_server`. Affects every variant name with an acronym.
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Msg { HTTPServer, URLParser, XMLHttp, ParseJSON }
        #[tauri::command]
        fn x() -> Result<Msg, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains(r#""http_server""#), "HTTPServer:\n{types}");
    assert!(types.contains(r#""url_parser""#), "URLParser:\n{types}");
    assert!(types.contains(r#""xml_http""#), "XMLHttp:\n{types}");
    assert!(types.contains(r#""parse_json""#), "ParseJSON:\n{types}");
}

#[test]
fn enum_with_rename_all_transforms_variants() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum Status { Active, InProgress, NotFound }
        #[tauri::command]
        fn x() -> Result<Status, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains(r#""ACTIVE""#), "{types}");
    assert!(types.contains(r#""IN_PROGRESS""#), "{types}");
    assert!(types.contains(r#""NOT_FOUND""#), "{types}");
}

#[test]
fn variant_rename_all_and_rename_all_fields_match_serde() {
    // Expected field names come from running serde_json on these enums.
    let types = types_for(
        r#"
        use serde::Serialize;

        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        pub enum Internal {
            #[serde(rename_all = "camelCase")]
            CurrentDate { zone: Option<String>, date_part: String },
            Plain { date_part: String },
        }

        #[derive(Serialize)]
        #[serde(tag = "t", content = "c", rename_all_fields = "camelCase")]
        pub enum Adjacent {
            A { date_part: String },
            #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
            B {
                date_part: String,
                #[serde(rename = "explicit")]
                other_field: i32,
            },
        }

        #[derive(Serialize)]
        #[serde(rename_all = "snake_case", rename_all_fields = "PascalCase")]
        pub enum External { SomeVariant { date_part: String } }

        #[derive(Serialize)]
        #[serde(untagged)]
        pub enum Untagged {
            #[serde(rename_all = "PascalCase")]
            A { date_part: String },
        }

        #[tauri::command]
        fn x() -> Result<(Internal, Adjacent, External, Untagged), String> { todo!() }
        "#,
    );

    for expected in [
        // {"type":"currentDate","zone":null,"datePart":"x"}
        r#"{ type: "currentDate", zone: string | null; datePart: string }"#,
        // {"type":"plain","date_part":"x"}
        r#"{ type: "plain", date_part: string }"#,
        // {"t":"A","c":{"datePart":"x"}}
        r#"{ t: "A"; c: { datePart: string } }"#,
        // {"t":"B","c":{"DATE_PART":"x","explicit":1}}
        r#"{ t: "B"; c: { DATE_PART: string; explicit: number } }"#,
        // {"some_variant":{"DatePart":"x"}}
        r#"{ some_variant: { DatePart: string } }"#,
        // {"DatePart":"x"}
        r#"{ DatePart: string }"#,
    ] {
        assert!(types.contains(expected), "missing `{expected}`:\n{types}");
    }
}

#[test]
fn non_identifier_property_keys_are_quoted() {
    let types = types_for(
        r#"
        use serde::Serialize;
        use tauri_ts_generator::TS;

        #[derive(Serialize, TS)]
        #[serde(rename_all = "kebab-case")]
        pub struct Headers {
            pub content_type: String,
            #[serde(rename = "x-spam")]
            #[ts(optional)]
            pub spam: Option<String>,
            #[serde(rename = "2fa")]
            pub two_factor: bool,
            #[serde(rename = "say \"hi\"")]
            pub quoted: i32,
            pub plain: i32,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "kebab-case", rename_all_fields = "SCREAMING-KEBAB-CASE")]
        pub enum External { SomeVariant { date_part: String }, Unit }

        #[derive(Serialize)]
        #[serde(tag = "msg-type", rename_all_fields = "kebab-case")]
        pub enum Internal { Ping { reply_to: String } }

        #[derive(Serialize)]
        #[serde(tag = "t", content = "the-content")]
        pub enum Adjacent { A(i32) }

        #[tauri::command]
        fn x() -> Result<(Headers, External, Internal, Adjacent), String> { todo!() }
        "#,
    );

    for expected in [
        r#""content-type": string;"#,
        r#""x-spam"?: string;"#,
        r#""2fa": boolean;"#,
        r#""say \"hi\"": number;"#,
        // Valid identifiers stay bare.
        "  plain: number;",
        r#"{ "some-variant": { "DATE-PART": string } }"#,
        r#"| "unit""#,
        r#"{ "msg-type": "Ping", "reply-to": string }"#,
        r#"{ t: "A"; "the-content": number }"#,
    ] {
        assert!(types.contains(expected), "missing `{expected}`:\n{types}");
    }
}
