//! Newtype variants of internally tagged enums.
//!
//! For `#[serde(tag = "type")] enum E { A(Inner) }` serde writes the tag
//! into the map `Inner` serializes to, so this only works when `Inner`
//! serializes as a JSON object. Otherwise serialization fails at runtime
//! with "cannot serialize tagged newtype variant E::A containing a string".
//! Observed with serde_json 1.x:
//!
//! | inner type                                   | result                         |
//! |----------------------------------------------|--------------------------------|
//! | named struct, map, newtype over either       | `{"type":"A", ...fields}`      |
//! | internally / adjacently tagged enum          | `{"type":"A", ...}`            |
//! | externally tagged enum                       | `{"type":"A","Variant":...}` — a unit variant gives `"Variant":null` |
//! | untagged enum                                | works for struct variants only |
//! | unit struct, `()`                            | `{"type":"A"}`                 |
//! | string, number, bool, `Option`, sequence, tuple, tuple struct | error |

use std::collections::{HashMap, HashSet};

use crate::known_types;
use crate::models::{
    EnumRepresentation, RustEnum, RustStruct, RustType, RustTypeAlias, StructShape, VariantData,
};
use crate::utils::simple_name;

/// What the inner type of an internally tagged newtype variant serializes to.
#[derive(Debug, PartialEq)]
pub(super) enum InnerShape {
    /// A JSON object; serde merges the tag into it.
    Object,
    /// An empty object (unit struct, `()`): only the tag remains.
    Empty,
    /// Never an object; serde fails at runtime. Says what it is instead,
    /// e.g. "a string".
    Rejected(String),
    /// An object for some values only, the type couldn't be analyzed, or
    /// the generated type doesn't describe what serde writes. Says why.
    Uncertain(String),
}

/// The generated structs, enums and aliases by name, to look up what a
/// newtype variant wraps.
#[derive(Default)]
pub(super) struct TypeIndex<'a> {
    structs: HashMap<&'a str, &'a RustStruct>,
    enums: HashMap<&'a str, &'a RustEnum>,
    aliases: HashMap<&'a str, &'a RustTypeAlias>,
}

impl<'a> TypeIndex<'a> {
    pub(super) fn new(
        structs: &'a [RustStruct],
        enums: &'a [RustEnum],
        aliases: &'a [RustTypeAlias],
    ) -> Self {
        Self {
            structs: structs.iter().map(|s| (s.name.as_str(), s)).collect(),
            enums: enums.iter().map(|e| (e.name.as_str(), e)).collect(),
            aliases: aliases.iter().map(|a| (a.name.as_str(), a)).collect(),
        }
    }

    pub(super) fn inner_shape(&self, ty: &RustType) -> InnerShape {
        self.shape(ty, &mut HashSet::new())
    }

    fn shape(&self, ty: &RustType, visiting: &mut HashSet<String>) -> InnerShape {
        match ty {
            RustType::Primitive(name) => primitive_shape(name),
            RustType::HashMap { .. } => InnerShape::Object,
            RustType::Unit => InnerShape::Empty,
            RustType::Vec(_) => InnerShape::Rejected("a sequence".into()),
            RustType::Tuple(_) => InnerShape::Rejected("a tuple".into()),
            // Even `Some(struct)`: serde sees the Option, not the struct.
            RustType::Option(_) => InnerShape::Rejected("an `Option`".into()),
            RustType::Result { .. } => InnerShape::Uncertain(
                "a `Result` serializes as `{\"Ok\": ...}`, which its generated type doesn't describe"
                    .into(),
            ),
            RustType::Channel(_) => {
                InnerShape::Uncertain("a `Channel` is not a serializable value".into())
            }
            RustType::Generic(name) => InnerShape::Uncertain(format!(
                "serde needs an object here, so generic parameter `{}` must be a struct or map",
                name
            )),
            RustType::Unknown(desc) => InnerShape::Uncertain(format!(
                "serde needs an object here, and `{}` couldn't be analyzed",
                desc
            )),
            RustType::Custom { name, .. } => self.custom_shape(simple_name(name), visiting),
        }
    }

    fn custom_shape(&self, name: &str, visiting: &mut HashSet<String>) -> InnerShape {
        if !visiting.insert(name.to_string()) {
            return InnerShape::Uncertain(format!(
                "serde needs an object here, and `{}` wraps itself",
                name
            ));
        }

        if let Some(s) = self.structs.get(name) {
            return match s.shape {
                StructShape::Named => InnerShape::Object,
                StructShape::Unit => InnerShape::Empty,
                StructShape::Newtype => match s.fields.first() {
                    Some(field) => self.shape(&field.ty, visiting),
                    None => InnerShape::Empty,
                },
                StructShape::Tuple => InnerShape::Rejected(format!("tuple struct `{}`", name)),
            };
        }

        if let Some(e) = self.enums.get(name) {
            return enum_shape(e);
        }

        if let Some(alias) = self.aliases.get(name) {
            return self.shape(&alias.target, visiting);
        }

        InnerShape::Uncertain(format!(
            "serde needs an object here, and `{}` is not among the generated types",
            name
        ))
    }
}

fn primitive_shape(name: &str) -> InnerShape {
    if name == known_types::JSON_VALUE_TYPE {
        return InnerShape::Uncertain(
            "serde needs an object here, so the `serde_json::Value` must hold one".into(),
        );
    }
    if name == known_types::BYTES_TYPE {
        return InnerShape::Rejected("a byte array".into());
    }
    if known_types::is_external_number_type(name) {
        // `Duration` serializes as `{ secs, nanos }` but is generated as
        // `number`, so the intersection would be wrong either way.
        return InnerShape::Uncertain(format!(
            "`{}`'s generated type doesn't describe its serialized form",
            name
        ));
    }
    match known_types::primitive_to_typescript(name) {
        Some("string") => InnerShape::Rejected("a string".into()),
        Some("number") => InnerShape::Rejected("a number".into()),
        Some("boolean") => InnerShape::Rejected("a boolean".into()),
        _ => InnerShape::Uncertain(format!(
            "serde needs an object here, and `{}` couldn't be analyzed",
            name
        )),
    }
}

fn enum_shape(e: &RustEnum) -> InnerShape {
    match &e.representation {
        EnumRepresentation::Internal { .. } | EnumRepresentation::Adjacent { .. } => {
            InnerShape::Object
        }
        EnumRepresentation::External => {
            if e.variants
                .iter()
                .any(|v| matches!(v.data, VariantData::Unit))
            {
                InnerShape::Uncertain(format!(
                    "unit variants of `{}` serialize as `{{\"Variant\": null}}` here, \
                    not as the string its generated type expects",
                    e.name
                ))
            } else {
                InnerShape::Object
            }
        }
        EnumRepresentation::Untagged => {
            if e.variants
                .iter()
                .all(|v| matches!(v.data, VariantData::Struct(_)))
            {
                InnerShape::Object
            } else {
                InnerShape::Uncertain(format!(
                    "serde needs an object here, which only the struct variants of \
                    untagged enum `{}` produce",
                    e.name
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EnumVariant, StructField};
    use std::path::PathBuf;

    fn field(ty: RustType) -> StructField {
        StructField {
            name: "0".into(),
            ty,
            has_explicit_rename: false,
            use_optional: false,
            is_flatten: false,
        }
    }

    fn strukt(name: &str, shape: StructShape, fields: Vec<StructField>) -> RustStruct {
        RustStruct {
            name: name.into(),
            generics: vec![],
            fields,
            shape,
            source_file: PathBuf::from("test.rs"),
        }
    }

    fn enumm(name: &str, representation: EnumRepresentation, data: Vec<VariantData>) -> RustEnum {
        RustEnum {
            name: name.into(),
            generics: vec![],
            variants: data
                .into_iter()
                .map(|data| EnumVariant {
                    name: "V".into(),
                    data,
                    has_explicit_rename: false,
                })
                .collect(),
            source_file: PathBuf::from("test.rs"),
            representation,
        }
    }

    fn is_uncertain(shape: InnerShape) -> bool {
        matches!(shape, InnerShape::Uncertain(_))
    }

    #[test]
    fn structs_by_shape() {
        let structs = vec![
            strukt("Named", StructShape::Named, vec![]),
            strukt("UnitS", StructShape::Unit, vec![]),
            strukt("Pair", StructShape::Tuple, vec![]),
            strukt(
                "OverNamed",
                StructShape::Newtype,
                vec![field(RustType::custom("Named"))],
            ),
            strukt(
                "OverString",
                StructShape::Newtype,
                vec![field(RustType::Primitive("String".into()))],
            ),
            strukt(
                "Loop",
                StructShape::Newtype,
                vec![field(RustType::custom("Loop"))],
            ),
        ];
        let index = TypeIndex::new(&structs, &[], &[]);
        let shape = |name: &str| index.inner_shape(&RustType::custom(name));

        assert_eq!(shape("Named"), InnerShape::Object);
        assert_eq!(shape("crate::model::Named"), InnerShape::Object);
        assert_eq!(shape("UnitS"), InnerShape::Empty);
        assert_eq!(
            shape("Pair"),
            InnerShape::Rejected("tuple struct `Pair`".into())
        );
        assert_eq!(shape("OverNamed"), InnerShape::Object);
        assert_eq!(shape("OverString"), InnerShape::Rejected("a string".into()));
        assert!(is_uncertain(shape("Loop")));
        assert!(is_uncertain(shape("Missing")));
    }

    #[test]
    fn enums_by_representation() {
        let struct_variant = || VariantData::Struct(vec![]);
        let enums = vec![
            enumm(
                "Internal",
                EnumRepresentation::Internal { tag: "t".into() },
                vec![VariantData::Unit],
            ),
            enumm(
                "Adjacent",
                EnumRepresentation::Adjacent {
                    tag: "t".into(),
                    content: "c".into(),
                },
                vec![VariantData::Unit],
            ),
            enumm(
                "ExtData",
                EnumRepresentation::External,
                vec![struct_variant(), VariantData::Tuple(vec![])],
            ),
            enumm(
                "ExtUnit",
                EnumRepresentation::External,
                vec![struct_variant(), VariantData::Unit],
            ),
            enumm(
                "UntaggedStructs",
                EnumRepresentation::Untagged,
                vec![struct_variant()],
            ),
            enumm(
                "UntaggedMixed",
                EnumRepresentation::Untagged,
                vec![struct_variant(), VariantData::Tuple(vec![])],
            ),
        ];
        let index = TypeIndex::new(&[], &enums, &[]);
        let shape = |name: &str| index.inner_shape(&RustType::custom(name));

        assert_eq!(shape("Internal"), InnerShape::Object);
        assert_eq!(shape("Adjacent"), InnerShape::Object);
        assert_eq!(shape("ExtData"), InnerShape::Object);
        assert!(is_uncertain(shape("ExtUnit")));
        assert_eq!(shape("UntaggedStructs"), InnerShape::Object);
        assert!(is_uncertain(shape("UntaggedMixed")));
    }

    #[test]
    fn builtin_types() {
        let index = TypeIndex::default();
        let prim = |name: &str| index.inner_shape(&RustType::Primitive(name.into()));
        let boxed = |ty: RustType| Box::new(ty);

        assert_eq!(prim("String"), InnerShape::Rejected("a string".into()));
        assert_eq!(prim("Uuid"), InnerShape::Rejected("a string".into()));
        assert_eq!(prim("u64"), InnerShape::Rejected("a number".into()));
        assert_eq!(prim("bool"), InnerShape::Rejected("a boolean".into()));
        assert_eq!(prim("Bytes"), InnerShape::Rejected("a byte array".into()));
        assert!(is_uncertain(prim("Value")));
        assert!(is_uncertain(prim("Duration")));

        let rule = || RustType::custom("Rule");
        assert_eq!(
            index.inner_shape(&RustType::Vec(boxed(rule()))),
            InnerShape::Rejected("a sequence".into())
        );
        assert_eq!(
            index.inner_shape(&RustType::Option(boxed(rule()))),
            InnerShape::Rejected("an `Option`".into())
        );
        assert_eq!(
            index.inner_shape(&RustType::Tuple(vec![rule(), rule()])),
            InnerShape::Rejected("a tuple".into())
        );
        assert_eq!(index.inner_shape(&RustType::Unit), InnerShape::Empty);
        assert_eq!(
            index.inner_shape(&RustType::HashMap {
                key: boxed(RustType::Primitive("String".into())),
                value: boxed(rule()),
            }),
            InnerShape::Object
        );
        assert!(is_uncertain(
            index.inner_shape(&RustType::Generic("T".into()))
        ));
    }

    #[test]
    fn aliases_are_followed() {
        let structs = vec![strukt("Named", StructShape::Named, vec![])];
        let aliases = vec![RustTypeAlias {
            name: "Alias".into(),
            generics: vec![],
            target: RustType::custom("Named"),
            source_file: PathBuf::from("test.rs"),
        }];
        let index = TypeIndex::new(&structs, &[], &aliases);
        assert_eq!(
            index.inner_shape(&RustType::custom("Alias")),
            InnerShape::Object
        );
    }
}
