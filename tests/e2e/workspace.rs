//! `source_dirs`: a Cargo workspace where commands live in the Tauri crate
//! and the types they use live in other crates. Every source dir is its own
//! crate root, so equally named modules (`crate::error` in two crates) must
//! not get mixed up.

use crate::helpers::{assert_contains, run_generate_err, run_generate_ok, Project};

fn workspace_project() -> Project {
    let project = Project::with_source(
        r#"
        mod error;
        use core_crate::model::Rule;
        use crate::error::Info;

        #[tauri::command]
        fn get_rule(id: i32) -> Result<Rule, String> { todo!() }

        #[tauri::command]
        fn get_info() -> Info { todo!() }
        "#,
    );
    project.add_source(
        "error.rs",
        r#"
        use serde::Serialize;
        #[derive(Serialize)]
        pub struct Info { pub app_version: String }
        "#,
    );

    project.add_file(
        "crates/core/Cargo.toml",
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\n",
    );
    project.add_file("crates/core/src/lib.rs", "pub mod error;\npub mod model;\n");
    project.add_file(
        "crates/core/src/model.rs",
        r#"
        use serde::Serialize;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Rule { pub rule_name: String, pub kind: crate::error::Kind }
        "#,
    );
    // Same module path as the app's `crate::error`, and an `Info` of its
    // own: a by-name lookup would find two `Info`s.
    project.add_file(
        "crates/core/src/error.rs",
        r#"
        use serde::Serialize;
        #[derive(Serialize)]
        pub enum Kind { Fatal, Warning }
        #[derive(Serialize)]
        pub struct Info { pub line: u32 }
        "#,
    );
    // Excluded like in a single-dir config.
    project.add_file(
        "crates/core/tests/model.rs",
        "#[tauri::command]\nfn test_only() {}\n",
    );

    // Commands are discovered in every source dir, not just the first one.
    project.add_file(
        "crates/plugin/src/lib.rs",
        r#"
        #[tauri::command]
        fn plugin_ping() -> String { todo!() }
        "#,
    );
    project
}

#[test]
fn source_dirs_discover_commands_and_types_in_all_crates() {
    let project = workspace_project();
    project.set_input_config(
        r#"source_dirs = ["src-tauri/src", "crates/core/src", "crates/plugin/src"]
exclude = ["tests"]"#,
    );
    run_generate_ok(&project);

    let commands = std::fs::read_to_string(&project.commands_out).unwrap();
    assert_contains(
        &commands,
        "export async function getRule(id: number): Promise<Rule>",
    );
    assert_contains(&commands, "export async function getInfo(): Promise<Info>");
    assert_contains(
        &commands,
        "export async function pluginPing(): Promise<string>",
    );
    assert!(
        !commands.contains("testOnly"),
        "excluded dir scanned:\n{commands}"
    );

    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert_contains(&types, "ruleName: string;");
    assert_contains(&types, "kind: Kind;");
    assert_contains(
        &types,
        "export type Kind =\n  | \"Fatal\"\n  | \"Warning\";",
    );
    // The app's own `Info`, not the core crate's.
    assert_contains(&types, "app_version: string;");
    assert!(!types.contains("line: number"), "wrong Info:\n{types}");
}

#[test]
fn source_dir_and_source_dirs_together_fail() {
    let project = workspace_project();
    project.set_input_config(
        r#"source_dir = "src-tauri/src"
source_dirs = ["crates/core/src"]"#,
    );
    let output = run_generate_err(&project);
    assert_contains(
        &String::from_utf8_lossy(&output.stderr),
        "Both `source_dir` and `source_dirs` are set",
    );
}
