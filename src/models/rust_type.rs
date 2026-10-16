/// Represents a Rust type
#[derive(Debug, Clone, PartialEq)]
pub enum RustType {
    /// Primitive types (String, i32, bool, etc.)
    Primitive(String),
    /// `Vec<T>`
    Vec(Box<RustType>),
    /// `Option<T>`
    Option(Box<RustType>),
    /// `Result<T, E>`. Only the Ok type appears in the TypeScript output;
    /// the error type is kept so its definition can still be generated.
    /// `err` is `None` for single-argument aliases like `io::Result<T>`.
    Result {
        ok: Box<RustType>,
        err: Option<Box<RustType>>,
    },
    /// `HashMap<K, V>`
    HashMap {
        key: Box<RustType>,
        value: Box<RustType>,
    },
    /// Tuple types
    Tuple(Vec<RustType>),
    /// `tauri::ipc::Channel<T>` — streaming channel passed from the frontend.
    /// Unlike injected types (State, Window), this IS present in the TS signature.
    Channel(Box<RustType>),
    /// Reference to a custom type (struct or enum), optionally with bound
    /// generic arguments. `Page<User>` is `Custom { name: "Page", args:
    /// [Custom { name: "User", args: vec![] }] }`. `args` is empty for
    /// references without generic parameters.
    Custom { name: String, args: Vec<RustType> },
    /// Generic type parameter (T, U, K, V, etc.)
    Generic(String),
    /// Unit type ()
    Unit,
    /// Unknown type (fallback)
    Unknown(String),
}

/// Walk a `RustType` tree, invoking `visit` for every `Custom(name)` node
/// that shows up in the TypeScript rendering of the type. The error side
/// of `Result<T, E>` is skipped (see [`walk_error_type_names`]).
///
/// Both the command collector (what types are reachable?) and the
/// import collector (what types are referenced in a generated file?)
/// need the same recursion; only the visitor action differs.
pub fn walk_custom_type_names<F: FnMut(&str)>(ty: &RustType, visit: &mut F) {
    match ty {
        RustType::Custom { name, args } => {
            visit(name);
            for arg in args {
                walk_custom_type_names(arg, visit);
            }
        }
        RustType::Vec(inner)
        | RustType::Option(inner)
        | RustType::Result { ok: inner, .. }
        | RustType::Channel(inner) => {
            walk_custom_type_names(inner, visit);
        }
        RustType::HashMap { key, value } => {
            walk_custom_type_names(key, visit);
            walk_custom_type_names(value, visit);
        }
        RustType::Tuple(types) => {
            for t in types {
                walk_custom_type_names(t, visit);
            }
        }
        RustType::Primitive(_) | RustType::Generic(_) | RustType::Unit | RustType::Unknown(_) => {}
    }
}

/// Walk a `RustType` tree, invoking `visit` for every `Custom(name)` node
/// inside the error type of a `Result<T, E>` — the names
/// [`walk_custom_type_names`] skips. Commands reject their promise with
/// the serialized error, so these types are worth generating even though
/// no signature mentions them.
pub fn walk_error_type_names<F: FnMut(&str)>(ty: &RustType, visit: &mut F) {
    match ty {
        RustType::Result { ok, err } => {
            walk_error_type_names(ok, visit);
            if let Some(err) = err {
                walk_custom_type_names(err, visit);
                walk_error_type_names(err, visit);
            }
        }
        RustType::Custom { args, .. } => {
            for arg in args {
                walk_error_type_names(arg, visit);
            }
        }
        RustType::Vec(inner) | RustType::Option(inner) | RustType::Channel(inner) => {
            walk_error_type_names(inner, visit);
        }
        RustType::HashMap { key, value } => {
            walk_error_type_names(key, visit);
            walk_error_type_names(value, visit);
        }
        RustType::Tuple(types) => {
            for t in types {
                walk_error_type_names(t, visit);
            }
        }
        RustType::Primitive(_) | RustType::Generic(_) | RustType::Unit | RustType::Unknown(_) => {}
    }
}

impl RustType {
    /// Shorthand for a non-generic `Custom` reference — avoids the
    /// `{ name: …, args: vec![] }` boilerplate at every construction site.
    pub fn custom(name: impl Into<String>) -> Self {
        RustType::Custom {
            name: name.into(),
            args: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn collect(ty: &RustType) -> Vec<String> {
        let mut set: HashSet<String> = HashSet::new();
        walk_custom_type_names(ty, &mut |n| {
            set.insert(n.to_string());
        });
        let mut out: Vec<String> = set.into_iter().collect();
        out.sort();
        out
    }

    #[test]
    fn simple_custom() {
        assert_eq!(collect(&RustType::custom("User")), vec!["User"]);
    }

    #[test]
    fn primitive_yields_nothing() {
        assert!(collect(&RustType::Primitive("String".into())).is_empty());
    }

    #[test]
    fn walks_into_vec() {
        let ty = RustType::Vec(Box::new(RustType::custom("Item")));
        assert_eq!(collect(&ty), vec!["Item"]);
    }

    #[test]
    fn walks_into_option() {
        let ty = RustType::Option(Box::new(RustType::custom("User")));
        assert_eq!(collect(&ty), vec!["User"]);
    }

    fn collect_errors(ty: &RustType) -> Vec<String> {
        let mut set: HashSet<String> = HashSet::new();
        walk_error_type_names(ty, &mut |n| {
            set.insert(n.to_string());
        });
        let mut out: Vec<String> = set.into_iter().collect();
        out.sort();
        out
    }

    fn result(ok: RustType, err: Option<RustType>) -> RustType {
        RustType::Result {
            ok: Box::new(ok),
            err: err.map(Box::new),
        }
    }

    #[test]
    fn walks_into_result() {
        let ty = result(RustType::custom("Response"), None);
        assert_eq!(collect(&ty), vec!["Response"]);
    }

    #[test]
    fn result_error_is_walked_separately() {
        let ty = result(
            RustType::custom("Response"),
            Some(RustType::Custom {
                name: "AppError".into(),
                args: vec![RustType::custom("Detail")],
            }),
        );
        assert_eq!(collect(&ty), vec!["Response"]);
        assert_eq!(collect_errors(&ty), vec!["AppError", "Detail"]);
    }

    #[test]
    fn error_walk_finds_results_nested_in_other_types() {
        // `AppResult<Result<User, Inner>>`-style nesting and Results in
        // generic args: every error type is found, no Ok-side name is.
        let ty = RustType::Custom {
            name: "Page".into(),
            args: vec![RustType::Vec(Box::new(result(
                result(RustType::custom("User"), Some(RustType::custom("Inner"))),
                Some(RustType::custom("Outer")),
            )))],
        };
        assert_eq!(collect_errors(&ty), vec!["Inner", "Outer"]);
        assert!(collect_errors(&result(RustType::custom("User"), None)).is_empty());
    }

    #[test]
    fn walks_into_hashmap_key_and_value() {
        let ty = RustType::HashMap {
            key: Box::new(RustType::Primitive("String".into())),
            value: Box::new(RustType::custom("User")),
        };
        assert_eq!(collect(&ty), vec!["User"]);
    }

    #[test]
    fn walks_into_tuple() {
        let ty = RustType::Tuple(vec![
            RustType::custom("User"),
            RustType::custom("Item"),
            RustType::Primitive("i32".into()),
        ]);
        assert_eq!(collect(&ty), vec!["Item", "User"]);
    }

    #[test]
    fn deduplicates_repeated_names() {
        let ty = RustType::Tuple(vec![RustType::custom("User"), RustType::custom("User")]);
        assert_eq!(collect(&ty), vec!["User"]);
    }

    #[test]
    fn nested_containers() {
        let ty = RustType::Vec(Box::new(RustType::Option(Box::new(RustType::Custom {
            name: "User".into(),
            args: Vec::new(),
        }))));
        assert_eq!(collect(&ty), vec!["User"]);
    }
}
