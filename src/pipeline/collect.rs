//! Walks command signatures and returns every struct/enum/alias definition
//! reachable from them. Conflicts (same name from two different source files)
//! and unresolved (macro-generated, probably) types are reported separately
//! so the caller can decide whether to bail or continue.
//!
//! The walk is a fixpoint: start from command args and return types, then
//! follow every nested custom type recorded in the resolver. `processed` and
//! `resolved_types` together prevent cycles and double-counting.
//!
//! Error types of command results (`Result<T, AppError>`, also through an
//! alias like `type AppResult<T> = Result<T, AppError>`) are seeded too, but
//! only when they derive `Serialize`; see `resolve_and_enqueue_error`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::diagnostics::Diagnostics;
use crate::models::{
    walk_custom_type_names, walk_error_type_names, RustEnum, RustStruct, RustType, RustTypeAlias,
    TauriCommand, VariantData,
};
use crate::parser::{parse_types, ParseOptions, ParsedTypes};
use crate::resolver::{ModuleResolver, ResolutionResult};
use crate::utils::simple_name;

/// Result of type collection with potential conflicts.
pub struct TypeCollectionResult {
    /// Collected structs reachable from command signatures
    pub structs: Vec<RustStruct>,
    /// Collected enums reachable from command signatures
    pub enums: Vec<RustEnum>,
    /// Collected type aliases reachable from command signatures
    pub aliases: Vec<RustTypeAlias>,
    /// Conflicts: type name -> list of conflicting source files
    pub conflicts: HashMap<String, Vec<PathBuf>>,
    /// Unresolved types: type name -> file where it was used
    pub unresolved: HashMap<String, PathBuf>,
}

/// Entry point: walk the command graph, return everything reachable.
pub fn collect_reachable_types(
    commands: &[TauriCommand],
    resolver: &ModuleResolver,
    expanded_types: Option<&ParsedTypes>,
    diag: &Diagnostics,
) -> TypeCollectionResult {
    let mut state = CollectState::new(resolver, diag);
    state.seed_expanded_types(expanded_types);
    state.seed_from_commands(commands);
    state.drain();
    state.finalize_reexport_aliases();
    state.into_result()
}

/// Walk every `RustType` in `roots`, collecting the names of every
/// `Custom` (or `CustomGeneric`) leaf into a sorted, deduped `Vec`.
///
/// One helper used by both the struct-field walk and the enum-variant
/// walk; the only difference is which `RustType`s feed in.
fn collect_custom<'a, I: IntoIterator<Item = &'a RustType>>(roots: I) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    for ty in roots {
        walk_custom_type_names(ty, &mut |name| {
            set.insert(name.to_string());
        });
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    out
}

fn custom_types(ty: &RustType) -> Vec<String> {
    collect_custom(std::iter::once(ty))
}

/// Names inside the error types of `Result`s in `ty`, sorted and deduped.
fn error_types(ty: &RustType) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    walk_error_type_names(ty, &mut |name| {
        set.insert(name.to_string());
    });
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    out
}

fn custom_types_from_variant(data: &VariantData) -> Vec<String> {
    match data {
        VariantData::Unit => Vec::new(),
        VariantData::Tuple(types) => collect_custom(types.iter()),
        VariantData::Struct(fields) => collect_custom(fields.iter().map(|f| &f.ty)),
    }
}

/// Mutable state threaded through the fixpoint walk. Collapsing the eight
/// previously-separate `&mut` arguments into one struct retires the
/// too-many-arguments lint exemption.
struct CollectState<'a> {
    resolver: &'a ModuleResolver,
    diag: &'a Diagnostics,

    structs: Vec<RustStruct>,
    enums: Vec<RustEnum>,
    aliases: Vec<RustTypeAlias>,
    conflicts: HashMap<String, Vec<PathBuf>>,
    unresolved: HashMap<String, PathBuf>,

    resolved_types: HashMap<String, PathBuf>,
    // Stored behind `Arc` so the drain loop can hand out a cheap handle
    // to the current file's parse tree without cloning the whole thing
    // on every iteration. Parsing is append-only within this collector,
    // so sharing is safe.
    parsed_files: HashMap<PathBuf, Arc<ParsedTypes>>,
    seen_structs: HashSet<(String, PathBuf)>,
    seen_enums: HashSet<(String, PathBuf)>,
    seen_aliases: HashSet<String>,
    reexport_aliases: HashMap<String, (String, PathBuf)>,
    to_process: Vec<(String, PathBuf)>,
    processed: HashSet<(String, PathBuf)>,
}

impl<'a> CollectState<'a> {
    fn new(resolver: &'a ModuleResolver, diag: &'a Diagnostics) -> Self {
        Self {
            resolver,
            diag,
            structs: Vec::new(),
            enums: Vec::new(),
            aliases: Vec::new(),
            conflicts: HashMap::new(),
            unresolved: HashMap::new(),
            resolved_types: HashMap::new(),
            parsed_files: HashMap::new(),
            seen_structs: HashSet::new(),
            seen_enums: HashSet::new(),
            seen_aliases: HashSet::new(),
            reexport_aliases: HashMap::new(),
            to_process: Vec::new(),
            processed: HashSet::new(),
        }
    }

    fn seed_expanded_types(&mut self, expanded_types: Option<&ParsedTypes>) {
        if let Some(parsed) = expanded_types {
            // The cargo-expand blob is seeded once; wrap it in an Arc
            // up front so subsequent drain-loop lookups are free.
            self.parsed_files
                .insert(PathBuf::from("<cargo-expand>"), Arc::new(parsed.clone()));
        }
    }

    fn add_conflict_path(&mut self, name: &str, path: &Path) {
        let entry = self.conflicts.entry(name.to_string()).or_default();
        if !entry.iter().any(|p| p == path) {
            entry.push(path.to_path_buf());
        }
    }

    /// Record `name` as resolving to `source`. If we already had a
    /// *different* source on file, report a conflict on both and return
    /// `false` (the caller may still want to queue further work such as
    /// tracking a re-export alias). Otherwise insert and return `true`.
    fn register_resolution(&mut self, name: &str, source: &Path) -> bool {
        if let Some(existing) = self.resolved_types.get(name).cloned() {
            if existing != source {
                self.add_conflict_path(name, &existing);
                self.add_conflict_path(name, source);
                return false;
            }
            return false;
        }
        self.resolved_types
            .insert(name.to_string(), source.to_path_buf());
        true
    }

    fn resolve_and_enqueue(&mut self, type_name: &str, from_file: &Path) {
        let resolution = self.resolver.resolve_type(type_name, from_file);
        self.enqueue_resolution(type_name, from_file, resolution);
    }

    /// Like `resolve_and_enqueue`, for the error type of a command result.
    ///
    /// Error types never appear in a generated signature, and the common
    /// Tauri pattern is a `thiserror` enum with a hand-written
    /// `impl Serialize` that sends a plain string. Its Rust shape says
    /// nothing about the JSON, so only types that *derive* `Serialize` are
    /// generated. Everything else (`String`, `tauri::Error`, hand-written
    /// impls, types that don't resolve) is skipped with a debug message
    /// only: error types used to be ignored entirely, so a warning would
    /// be new noise for projects that work fine.
    fn resolve_and_enqueue_error(&mut self, type_name: &str, from_file: &Path) {
        let resolution = self.resolver.resolve_type(type_name, from_file);
        let definition = match &resolution {
            ResolutionResult::Found(source) => Some((simple_name(type_name), source)),
            ResolutionResult::FoundWithAlias(source, original) => Some((original.as_str(), source)),
            ResolutionResult::NotFound | ResolutionResult::Ambiguous(_) => None,
        };
        let derives_serialize = definition.is_some_and(|(name, source)| {
            self.ensure_parsed(source)
                && self
                    .parsed_files
                    .get(source)
                    .is_some_and(|parsed| parsed.derives_serialize.contains(name))
        });

        if derives_serialize {
            self.enqueue_resolution(type_name, from_file, resolution);
        } else {
            self.diag.debug(format!(
                "Not generating error type '{}' (used in {}): no #[derive(Serialize)] definition found",
                type_name,
                from_file.display()
            ));
        }
    }

    fn enqueue_resolution(
        &mut self,
        type_name: &str,
        from_file: &Path,
        resolution: ResolutionResult,
    ) {
        let name = simple_name(type_name).to_string();

        match resolution {
            ResolutionResult::Found(source) => {
                if self.register_resolution(&name, &source) {
                    self.to_process.push((name, source));
                }
            }
            ResolutionResult::FoundWithAlias(source, original_name) => {
                // Alias resolutions always need to chase the original
                // definition, even when the alias name is already known.
                self.register_resolution(&name, &source);
                self.reexport_aliases
                    .entry(name)
                    .or_insert_with(|| (original_name.clone(), source.clone()));
                self.to_process.push((original_name, source));
            }
            ResolutionResult::Ambiguous(paths) => {
                for path in paths {
                    self.add_conflict_path(&name, &path);
                }
            }
            ResolutionResult::NotFound => {
                self.unresolved
                    .entry(name)
                    .or_insert_with(|| from_file.to_path_buf());
            }
        }
    }

    fn seed_from_commands(&mut self, commands: &[TauriCommand]) {
        for cmd in commands {
            let cmd_file = cmd.source_file.clone();
            for arg in &cmd.args {
                for t in custom_types(&arg.ty) {
                    self.resolve_and_enqueue(&t, &cmd_file);
                }
            }
            if let Some(ret_type) = &cmd.return_type {
                for t in custom_types(ret_type) {
                    self.resolve_and_enqueue(&t, &cmd_file);
                }
                for t in error_types(ret_type) {
                    self.resolve_and_enqueue_error(&t, &cmd_file);
                }
            }
        }
    }

    /// Lazily read+parse a source file on first reference. Returns `true`
    /// when the file is available in `parsed_files` afterwards.
    fn ensure_parsed(&mut self, type_file: &Path) -> bool {
        if self.parsed_files.contains_key(type_file) {
            return true;
        }
        let content = match fs::read_to_string(type_file) {
            Ok(c) => c,
            Err(e) => {
                self.diag.warn(format!(
                    "Failed to read file for types {}: {}",
                    type_file.display(),
                    e
                ));
                return false;
            }
        };
        match parse_types(&content, type_file, ParseOptions::SOURCE_ALL) {
            Ok(parsed) => {
                self.parsed_files
                    .insert(type_file.to_path_buf(), Arc::new(parsed));
                true
            }
            Err(e) => {
                self.diag.warn(format!(
                    "Failed to parse types in {}: {}",
                    type_file.display(),
                    e
                ));
                false
            }
        }
    }

    fn drain(&mut self) {
        let expanded_path = PathBuf::from("<cargo-expand>");
        while let Some((type_name, type_file)) = self.to_process.pop() {
            let key = (type_name.clone(), type_file.clone());
            if !self.processed.insert(key) {
                continue;
            }

            if type_file != expanded_path && !self.ensure_parsed(&type_file) {
                continue;
            }

            // Cheap `Arc::clone` of the shared parse tree — same handle,
            // no bulk data copy. Lets the body below borrow into `parsed`
            // while still calling `&mut self` methods like
            // `resolve_and_enqueue` without borrow conflicts.
            let parsed = match self.parsed_files.get(&type_file) {
                Some(p) => Arc::clone(p),
                None => continue,
            };

            if let Some(s) = parsed.structs.iter().find(|s| s.name == type_name) {
                if self
                    .seen_structs
                    .insert((s.name.clone(), type_file.clone()))
                {
                    self.structs.push(s.clone());
                }
                for field in &s.fields {
                    for t in custom_types(&field.ty) {
                        self.resolve_and_enqueue(&t, &type_file);
                    }
                }
                continue;
            }

            if let Some(e) = parsed.enums.iter().find(|e| e.name == type_name) {
                if self.seen_enums.insert((e.name.clone(), type_file.clone())) {
                    self.enums.push(e.clone());
                }
                for variant in &e.variants {
                    for t in custom_types_from_variant(&variant.data) {
                        self.resolve_and_enqueue(&t, &type_file);
                    }
                }
                continue;
            }

            if let Some(alias) = parsed.aliases.iter().find(|a| a.name == type_name) {
                if self.seen_aliases.insert(alias.name.clone()) {
                    self.aliases.push(alias.clone());
                }
                let alias_source = alias.source_file.clone();
                for t in custom_types(&alias.target) {
                    self.resolve_and_enqueue(&t, &alias_source);
                }
                // `type AppResult<T> = Result<T, AppError>` renders as
                // `T`, but the error type still needs generating.
                for t in error_types(&alias.target) {
                    self.resolve_and_enqueue_error(&t, &alias_source);
                }
            }
        }
    }

    fn finalize_reexport_aliases(&mut self) {
        // Drain the re-export map so self is free to mutate `self.aliases`.
        let reexports: Vec<(String, (String, PathBuf))> = self.reexport_aliases.drain().collect();
        for (alias_name, (original_name, source_file)) in reexports {
            if self.seen_aliases.contains(&alias_name) {
                continue;
            }
            let generics = self
                .parsed_files
                .get(&source_file)
                .and_then(|parsed| {
                    parsed
                        .structs
                        .iter()
                        .find(|s| s.name == original_name)
                        .map(|s| s.generics.clone())
                        .or_else(|| {
                            parsed
                                .enums
                                .iter()
                                .find(|e| e.name == original_name)
                                .map(|e| e.generics.clone())
                        })
                })
                .unwrap_or_default();

            self.aliases.push(RustTypeAlias {
                name: alias_name,
                generics,
                target: RustType::Custom {
                    name: original_name,
                    args: Vec::new(),
                },
                source_file,
            });
        }
    }

    fn into_result(self) -> TypeCollectionResult {
        TypeCollectionResult {
            structs: self.structs,
            enums: self.enums,
            aliases: self.aliases,
            conflicts: self.conflicts,
            unresolved: self.unresolved,
        }
    }
}
