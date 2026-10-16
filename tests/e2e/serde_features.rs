//! `#[serde(flatten)]`, `#[serde(skip)]`, `#[serde(tag=...)]`, `#[serde(untagged)]`.

use crate::helpers::{run_generate_ok, Project};

#[test]
fn flatten_produces_typescript_intersection() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub struct Address { pub city: String, pub country: String }

        #[derive(Serialize, Deserialize)]
        pub struct User {
            pub name: String,
            #[serde(flatten)]
            pub address: Address,
        }

        #[tauri::command]
        fn x() -> Result<User, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains("export interface Address"));
    // Flatten produces a `type X = { … } & Address` form.
    assert!(
        types.contains("export type User") && types.contains("} & Address"),
        "expected intersection type:\n{types}"
    );
}

#[test]
fn skip_excludes_field() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub struct User {
            pub id: i32,
            #[serde(skip)]
            pub internal: String,
        }

        #[tauri::command]
        fn x() -> Result<User, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains("id: number"));
    assert!(
        !types.contains("internal:"),
        "#[serde(skip)] field must be excluded:\n{types}"
    );
}

#[test]
fn skip_serializing_is_kept_because_input_path_still_needs_it() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub struct User {
            pub id: i32,
            #[serde(skip_serializing)]
            pub password: String,
        }

        #[tauri::command]
        fn x() -> Result<User, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains("password: string"), "{types}");
}

#[test]
fn tag_content_adjacent_enum() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        #[serde(tag = "kind", content = "data")]
        pub enum Event {
            Click { x: i32, y: i32 },
            KeyPress(String),
            Idle,
        }

        #[tauri::command]
        fn x() -> Result<Event, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    // Tag key is emitted unquoted in TypeScript object-type syntax.
    assert!(
        types.contains(r#"kind: "Click""#),
        "Click variant:\n{types}"
    );
    assert!(
        types.contains(r#"kind: "KeyPress""#),
        "KeyPress variant:\n{types}"
    );
    assert!(types.contains(r#"kind: "Idle""#), "Idle variant:\n{types}");
    // Content key must only appear for variants with payload, not the unit one.
    assert!(types.contains("data:"));
}

#[test]
fn flatten_of_plain_enum_emits_warning() {
    // `#[serde(flatten)]` on a field whose type is a plain external-tagged
    // enum like `enum Role { Admin, User }` is a runtime footgun: serde
    // serializes the enum as a string, flatten needs a map, and the TS
    // intersection `{ … } & "Admin" | "User"` reduces to `never`. The
    // generator must surface a warning so the user notices before they
    // hit production.
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub enum Role { Admin, User }

        #[derive(Serialize, Deserialize)]
        pub struct Account {
            pub name: String,
            #[serde(flatten)]
            pub role: Role,
        }

        #[tauri::command]
        fn get() -> Result<Account, String> { todo!() }
        "#,
    );
    let output = crate::helpers::run_generate_ok(&project);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("flatten") && stderr.contains("Role") && stderr.contains("enum"),
        "expected warning mentioning the flatten-on-enum misuse, got:\n{stderr}"
    );
}

#[test]
fn untagged_enum_produces_union_of_payloads() {
    let project = Project::with_source(
        r#"
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        pub struct Msg { pub body: String }

        #[derive(Serialize, Deserialize)]
        #[serde(untagged)]
        pub enum Envelope {
            Text(String),
            Wrapped(Msg),
        }

        #[tauri::command]
        fn x() -> Result<Envelope, String> { todo!() }
        "#,
    );
    run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(types.contains("export type Envelope"), "{types}");
    // Untagged should NOT include a tag key.
    assert!(
        !types.contains(r#""kind":"#) && !types.contains(r#""type":"#),
        "untagged must not add a tag:\n{types}"
    );
    assert!(types.contains("string") && types.contains("Msg"));
}

#[test]
fn internally_tagged_newtype_variants_warn_only_when_serde_fails() {
    // serde merges the tag into the inner value's map. Expected shapes and
    // failures were checked with serde_json.
    let project = Project::with_source(
        r#"
        use serde::Serialize;
        use std::collections::HashMap;

        #[derive(Serialize)]
        pub struct Rule { pub name: String }
        #[derive(Serialize)]
        pub struct Marker;
        #[derive(Serialize)]
        pub struct Wrapped(Rule);
        #[derive(Serialize)]
        #[serde(tag = "kind")]
        pub enum Inner { A { x: i32 } }
        #[derive(Serialize)]
        pub enum Plain { On, Off }

        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "camelCase")]
        pub enum Item {
            Rule(Rule),
            Marker(Marker),
            Wrapped(Wrapped),
            Map(HashMap<String, i32>),
            Inner(Inner),
            Text(String),
            List(Vec<Rule>),
            Maybe(Option<Rule>),
            Plain(Plain),
        }

        #[tauri::command]
        fn get() -> Item { todo!() }
        "#,
    );
    let output = run_generate_ok(&project);
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    for expected in [
        r#"({ type: "rule" } & Rule)"#,
        // `{"type":"marker"}`: a unit struct adds no fields, and
        // `{ … } & null` would be `never`.
        r#"| { type: "marker" }"#,
        r#"({ type: "wrapped" } & Wrapped)"#,
        r#"({ type: "map" } & Record<string, number>)"#,
        r#"({ type: "inner" } & Inner)"#,
        // Best effort for a variant serde rejects; parenthesized so `&`
        // doesn't bind tighter than `|`.
        r#"({ type: "maybe" } & (Rule | null))"#,
    ] {
        assert!(types.contains(expected), "missing `{expected}`:\n{types}");
    }

    // Object-like payloads are fine: no warning for them.
    for ok in ["'rule'", "'marker'", "'wrapped'", "'map'", "'inner'"] {
        assert!(
            !stderr.contains(ok),
            "unexpected warning for {ok}:\n{stderr}"
        );
    }
    // These fail in serde_json ("cannot serialize tagged newtype variant").
    for (variant, what) in [
        ("'text'", "wraps a string"),
        ("'list'", "wraps a sequence"),
        ("'maybe'", "wraps an `Option`"),
    ] {
        assert!(
            stderr
                .lines()
                .any(|l| l.contains(variant) && l.contains(what) && l.contains("fails at runtime")),
            "expected a runtime-failure warning for {variant}:\n{stderr}"
        );
    }
    // Serializes, but unit variants come out as `{"type":"plain","On":null}`,
    // which the generated `"On" | "Off"` doesn't describe.
    assert!(
        stderr
            .lines()
            .any(|l| l.contains("'plain'") && l.contains("unit variants of `Plain`")),
        "expected a warning for 'plain':\n{stderr}"
    );
}
