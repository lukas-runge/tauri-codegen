//! Error types of command results. A command returning `Result<T, E>`
//! rejects its promise with the serialized `E`, so `E` (and everything it
//! references) belongs in the types file even though no generated
//! signature mentions it. Signatures keep returning `Promise<T>`.

use crate::helpers::{assert_contains, run_generate_ok, Project};

const ERROR_MODULE: &str = r#"
    use serde::Serialize;

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub enum ErrorKind { NotFound, Io }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AppError { pub kind: ErrorKind, pub message: String }

    pub type AppResult<T> = Result<T, AppError>;
"#;

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn direct_result_error_type_is_generated() {
    let project = Project::with_source(
        r#"
        mod error;
        use error::AppError;

        #[tauri::command]
        fn get_name() -> Result<String, AppError> { todo!() }
        "#,
    );
    project.add_source("error.rs", ERROR_MODULE);
    run_generate_ok(&project);

    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert_contains(
        &types,
        "export interface AppError {\n  kind: ErrorKind;\n  message: string;\n}",
    );
    assert_contains(
        &types,
        "export type ErrorKind =\n  | \"notFound\"\n  | \"io\";",
    );

    // The signature is unchanged, and `AppError` is not imported: an
    // import used by no signature fails `noUnusedLocals`.
    let commands = std::fs::read_to_string(&project.commands_out).unwrap();
    assert_contains(
        &commands,
        "export async function getName(): Promise<string> {",
    );
    assert!(!commands.contains("AppError"), "{commands}");
}

#[test]
fn error_type_behind_result_alias_is_generated() {
    let project = Project::with_source(
        r#"
        mod error;
        use error::AppResult;
        use serde::Serialize;

        #[derive(Serialize)]
        pub struct User { pub id: i32 }

        #[tauri::command]
        fn list_users() -> AppResult<Vec<User>> { todo!() }
        "#,
    );
    project.add_source("error.rs", ERROR_MODULE);
    run_generate_ok(&project);

    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert_contains(&types, "export type AppResult<T> = T;");
    assert_contains(&types, "export interface AppError {");
    assert_contains(&types, "export type ErrorKind =");

    let commands = std::fs::read_to_string(&project.commands_out).unwrap();
    assert_contains(
        &commands,
        "export async function listUsers(): Promise<AppResult<User[]>> {",
    );
    assert_contains(&commands, "import type { AppResult, User } from");
}

#[test]
fn error_type_from_another_source_dir_is_generated() {
    let project = Project::with_source(
        r#"
        #[tauri::command]
        fn ping() -> Result<(), core_crate::error::AppError> { todo!() }
        "#,
    );
    project.add_file(
        "crates/core/Cargo.toml",
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\n",
    );
    project.add_file("crates/core/src/lib.rs", "pub mod error;\n");
    project.add_file("crates/core/src/error.rs", ERROR_MODULE);
    project.set_input_config(r#"source_dirs = ["src-tauri/src", "crates/core/src"]"#);
    run_generate_ok(&project);

    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert_contains(&types, "export interface AppError {");
    assert_contains(&types, "export type ErrorKind =");
}

#[test]
fn unsupported_error_types_are_skipped_quietly() {
    // None of these has a shape the generator can describe: primitives,
    // foreign types, trait objects, and the `thiserror` + hand-written
    // `impl Serialize` pattern from the Tauri docs (serializes as a string,
    // whatever the enum looks like).
    let project = Project::with_source(
        r#"
        #[derive(Debug, thiserror::Error)]
        pub enum Error {
            #[error(transparent)]
            Io(#[from] std::io::Error),
        }

        impl serde::Serialize for Error {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.to_string().as_ref())
            }
        }

        #[tauri::command]
        fn a() -> Result<(), Error> { todo!() }
        #[tauri::command]
        fn b() -> Result<(), String> { todo!() }
        #[tauri::command]
        fn c() -> Result<(), tauri::Error> { todo!() }
        #[tauri::command]
        fn d() -> Result<(), Box<dyn std::error::Error>> { todo!() }
        #[tauri::command]
        fn e() -> Result<(), anyhow::Error> { todo!() }
        "#,
    );
    let output = run_generate_ok(&project);

    let stderr = stderr_of(&output);
    assert!(
        !stderr.contains("could not be resolved") && !stderr.contains("Unknown type"),
        "error types must not produce warnings:\n{stderr}"
    );
    let types = std::fs::read_to_string(&project.types_out).unwrap();
    assert!(!types.contains("Error"), "{types}");
}
